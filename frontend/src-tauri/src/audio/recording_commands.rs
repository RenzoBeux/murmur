// audio/recording_commands.rs
//
// Slim Tauri command layer for recording functionality.
// Delegates to transcription and recording modules for actual implementation.

use anyhow::Result;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::task::JoinHandle;

use super::{
    parse_audio_device,
    default_input_device,   // Get default microphone
    default_output_device,  // Get default system audio
    RecordingManager,
    DeviceEvent,
    DeviceMonitorType
};

// Import transcription modules
use super::transcription::{
    self,
    reset_speech_detected_flag,
};

// Re-export TranscriptUpdate for backward compatibility
pub use super::transcription::TranscriptUpdate;

// ============================================================================
// GLOBAL STATE
// ============================================================================

// Simple recording state tracking
static IS_RECORDING: AtomicBool = AtomicBool::new(false);

// Global recording manager and transcription task to keep them alive during recording
static RECORDING_MANAGER: Mutex<Option<RecordingManager>> = Mutex::new(None);
static TRANSCRIPTION_TASK: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

// Listener ID for proper cleanup - prevents microphone from staying active after recording stops
static TRANSCRIPT_LISTENER_ID: Mutex<Option<tauri::EventId>> = Mutex::new(None);

// ============================================================================
// PUBLIC TYPES
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct RecordingArgs {
    pub save_path: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct TranscriptionStatus {
    pub chunks_in_queue: usize,
    pub is_processing: bool,
    pub last_activity_ms: u64,
}

// ============================================================================
// RECORDING COMMANDS
// ============================================================================

/// Minimum free disk space (bytes) required to start a recording. Below this we
/// refuse to start rather than record into a volume that fills mid-meeting and
/// silently truncates the audio/transcript.
const MIN_FREE_DISK_BYTES: u64 = 500 * 1024 * 1024; // 500 MB

/// Abort start if the volume holding the app data dir is critically low on space.
/// Best-effort: if we cannot resolve the path or the disk, we ALLOW the recording
/// (never block the user on our own uncertainty). Matches the disk whose mount
/// point is the longest prefix of the target path.
fn check_free_disk_space<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let target = match app.path().app_data_dir() {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut best: Option<(usize, u64)> = None; // (mount_point_len, available_bytes)
    for disk in &disks {
        let mount = disk.mount_point();
        if target.starts_with(mount) {
            let len = mount.as_os_str().len();
            if best.map_or(true, |(blen, _)| len > blen) {
                best = Some((len, disk.available_space()));
            }
        }
    }
    if let Some((_, available)) = best {
        if available < MIN_FREE_DISK_BYTES {
            return Err(format!(
                "Not enough free disk space to start recording: {} MB available, at least {} MB required. Free up space and try again.",
                available / (1024 * 1024),
                MIN_FREE_DISK_BYTES / (1024 * 1024)
            ));
        }
    }
    Ok(())
}

/// Start recording with default devices
pub async fn start_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    start_recording_with_meeting_name(app, None).await
}

/// Start recording with default devices and optional meeting name
pub async fn start_recording_with_meeting_name<R: Runtime>(
    app: AppHandle<R>,
    meeting_name: Option<String>,
) -> Result<(), String> {
    info!(
        "Starting recording with default devices, meeting: {:?}",
        meeting_name
    );

    // Check if already recording
    let current_recording_state = IS_RECORDING.load(Ordering::SeqCst);
    info!("🔍 IS_RECORDING state check: {}", current_recording_state);
    if current_recording_state {
        return Err("Recording already in progress".to_string());
    }

    // Refuse to start on a critically-full disk (prevents mid-meeting truncation).
    check_free_disk_space(&app)?;

    // Validate that transcription models are available before starting recording
    info!("🔍 Validating transcription model availability before starting recording...");
    if let Err(validation_error) = transcription::validate_transcription_model_ready(&app).await {
        error!("Model validation failed: {}", validation_error);

        // Emit error event for frontend - actionable: false to show toast instead of modal
        // (download progress is already shown in top-right toast)
        let _ = app.emit("transcription-error", serde_json::json!({
            "error": validation_error,
            "userMessage": "Recording cannot start: Transcription model is still downloading. Please wait for the download to complete.",
            "actionable": false
        }));

        return Err(validation_error);
    }
    info!("✅ Transcription model validation passed");

    // Async-first approach - no more blocking operations!
    info!("🚀 Starting async recording initialization");

    // Create new recording manager
    let mut manager = RecordingManager::new();

    // Load recording preferences to get auto_save AND device preferences
    let (auto_save, preferred_mic_name, preferred_system_name) =
        match super::recording_preferences::load_recording_preferences(&app).await {
            Ok(prefs) => {
                info!("📋 Loaded recording preferences: auto_save={}, preferred_mic={:?}, preferred_system={:?}",
                      prefs.auto_save, prefs.preferred_mic_device, prefs.preferred_system_device);
                (prefs.auto_save, prefs.preferred_mic_device, prefs.preferred_system_device)
            }
            Err(e) => {
                warn!("Failed to load recording preferences, using defaults: {}", e);
                (true, None, None)
            }
        };

    // ============================================================================
    // MICROPHONE DEVICE RESOLUTION: Preference → Default → Error
    // ============================================================================
    let microphone_device = match preferred_mic_name {
        Some(pref_name) => {
            info!("🎤 Attempting to use preferred microphone: '{}'", pref_name);
            match parse_audio_device(&pref_name) {
                Ok(device) => {
                    info!("✅ Using preferred microphone: '{}'", device.name);
                    Some(Arc::new(device))
                }
                Err(e) => {
                    warn!("⚠️ Preferred microphone '{}' not available: {}", pref_name, e);
                    warn!("   Falling back to system default microphone...");
                    match default_input_device() {
                        Ok(device) => {
                            info!("✅ Using default microphone: '{}'", device.name);
                            Some(Arc::new(device))
                        }
                        Err(default_err) => {
                            error!("❌ No microphone available (preferred and default both failed)");
                            return Err(format!(
                                "No microphone device available. Preferred device '{}' not found, and default microphone unavailable: {}",
                                pref_name, default_err
                            ));
                        }
                    }
                }
            }
        }
        None => {
            info!("🎤 No microphone preference set, using system default");
            match default_input_device() {
                Ok(device) => {
                    info!("✅ Using default microphone: '{}'", device.name);
                    Some(Arc::new(device))
                }
                Err(e) => {
                    error!("❌ No default microphone available");
                    return Err(format!("No microphone device available: {}", e));
                }
            }
        }
    };

    // ============================================================================
    // SYSTEM AUDIO DEVICE RESOLUTION: Preference → Default → None (optional)
    // ============================================================================
    let system_device = match preferred_system_name {
        Some(pref_name) => {
            info!("🔊 Attempting to use preferred system audio: '{}'", pref_name);
            match parse_audio_device(&pref_name) {
                Ok(device) => {
                    info!("✅ Using preferred system audio: '{}'", device.name);
                    Some(Arc::new(device))
                }
                Err(e) => {
                    warn!("⚠️ Preferred system audio '{}' not available: {}", pref_name, e);
                    warn!("   Falling back to system default...");
                    match default_output_device() {
                        Ok(device) => {
                            info!("✅ Using default system audio: '{}'", device.name);
                            Some(Arc::new(device))
                        }
                        Err(default_err) => {
                            warn!("⚠️ No system audio available (preferred and default both failed): {}", default_err);
                            warn!("   Recording will continue with microphone only");
                            None // System audio is optional
                        }
                    }
                }
            }
        }
        None => {
            info!("🔊 No system audio preference set, using system default");
            match default_output_device() {
                Ok(device) => {
                    info!("✅ Using default system audio: '{}'", device.name);
                    Some(Arc::new(device))
                }
                Err(e) => {
                    warn!("⚠️ No default system audio available: {}", e);
                    warn!("   Recording will continue with microphone only");
                    None // System audio is optional
                }
            }
        }
    };

    // Always ensure a meeting name is set so incremental saver initializes
    let effective_meeting_name = meeting_name.clone().unwrap_or_else(|| {
        // Example: Meeting 2025-10-03_08-25-23
        let now = chrono::Local::now();
        format!(
            "Meeting {}",
            now.format("%Y-%m-%d_%H-%M-%S")
        )
    });
    manager.set_meeting_name(Some(effective_meeting_name));

    // Set up error callback
    let app_for_error = app.clone();
    manager.set_error_callback(move |error| {
        let _ = app_for_error.emit("recording-error", error.user_message());
    });

    // Start recording with resolved devices (replaces start_recording_with_defaults_and_auto_save call)
    let transcription_receiver = manager
        .start_recording(microphone_device, system_device, auto_save)
        .await
        .map_err(|e| format!("Failed to start recording: {}", e))?;

    // Store the manager globally to keep it alive
    {
        let mut global_manager = RECORDING_MANAGER.lock().unwrap();
        *global_manager = Some(manager);
    }

    // Set recording flag and reset speech detection flag
    info!("🔍 Setting IS_RECORDING to true and resetting SPEECH_DETECTED_EMITTED");
    IS_RECORDING.store(true, Ordering::SeqCst);
    reset_speech_detected_flag(); // Reset for new recording session

    // Start optimized parallel transcription task and store handle
    let task_handle = transcription::start_transcription_task(app.clone(), transcription_receiver);
    {
        let mut global_task = TRANSCRIPTION_TASK.lock().unwrap();
        *global_task = Some(task_handle);
    }

    // CRITICAL: Listen for transcript-update events and save to recording manager
    // This enables transcript history persistence for page reload sync
    // Store listener ID for cleanup during stop_recording to ensure microphone is released
    {
        use tauri::Listener;
        let listener_id = app.listen("transcript-update", move |event: tauri::Event| {
            // Parse the transcript update from the event payload
            if let Ok(update) = serde_json::from_str::<TranscriptUpdate>(event.payload()) {
                // Create structured transcript segment
                let segment = crate::audio::recording_saver::TranscriptSegment {
                    id: format!("seg_{}", update.sequence_id),
                    text: update.text.clone(),
                    audio_start_time: update.audio_start_time,
                    audio_end_time: update.audio_end_time,
                    duration: update.duration,
                    display_time: update.timestamp.clone(), // Use wall-clock timestamp for display
                    confidence: update.confidence,
                    sequence_id: update.sequence_id,
                    speaker: update.speaker.clone(),
                };

                // Save to recording manager
                if let Ok(manager_guard) = RECORDING_MANAGER.lock() {
                    if let Some(manager) = manager_guard.as_ref() {
                        manager.add_transcript_segment(segment);
                    }
                }
            }
        });
        let mut global_listener = TRANSCRIPT_LISTENER_ID.lock().unwrap();
        *global_listener = Some(listener_id);
        info!("✅ Transcript-update event listener registered for history persistence");
    }

    // Emit success event
    app.emit("recording-started", serde_json::json!({
        "message": "Recording started successfully with parallel processing",
        "devices": ["Default Microphone", "Default System Audio"],
        "workers": 3
    })).map_err(|e| e.to_string())?;

    // Update tray menu to reflect recording state
    crate::tray::update_tray_menu(&app);

    info!("✅ Recording started successfully with async-first approach");

    Ok(())
}

/// Start recording with specific devices
pub async fn start_recording_with_devices<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
) -> Result<(), String> {
    start_recording_with_devices_and_meeting(app, mic_device_name, system_device_name, None).await
}

/// Start recording with specific devices and optional meeting name
pub async fn start_recording_with_devices_and_meeting<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
    meeting_name: Option<String>,
) -> Result<(), String> {
    info!(
        "Starting recording with specific devices: mic={:?}, system={:?}, meeting={:?}",
        mic_device_name, system_device_name, meeting_name
    );

    // Check if already recording
    let current_recording_state = IS_RECORDING.load(Ordering::SeqCst);
    info!("🔍 IS_RECORDING state check: {}", current_recording_state);
    if current_recording_state {
        return Err("Recording already in progress".to_string());
    }

    // Refuse to start on a critically-full disk (prevents mid-meeting truncation).
    check_free_disk_space(&app)?;

    // Validate that transcription models are available before starting recording
    info!("🔍 Validating transcription model availability before starting recording...");
    if let Err(validation_error) = transcription::validate_transcription_model_ready(&app).await {
        error!("Model validation failed: {}", validation_error);

        // Emit error event for frontend - actionable: false to show toast instead of modal
        // (download progress is already shown in top-right toast)
        let _ = app.emit("transcription-error", serde_json::json!({
            "error": validation_error,
            "userMessage": "Recording cannot start: Transcription model is still downloading. Please wait for the download to complete.",
            "actionable": false
        }));

        return Err(validation_error);
    }
    info!("✅ Transcription model validation passed");

    // Parse devices
    let mic_device = if let Some(ref name) = mic_device_name {
        Some(Arc::new(parse_audio_device(name).map_err(|e| {
            format!("Invalid microphone device '{}': {}", name, e)
        })?))
    } else {
        None
    };

    let system_device = if let Some(ref name) = system_device_name {
        Some(Arc::new(parse_audio_device(name).map_err(|e| {
            format!("Invalid system device '{}': {}", name, e)
        })?))
    } else {
        None
    };

    // Async-first approach for custom devices - no more blocking operations!
    info!("🚀 Starting async recording initialization with custom devices");

    // Create new recording manager
    let mut manager = RecordingManager::new();

    // Load recording preferences to check auto_save setting
    let auto_save = match super::recording_preferences::load_recording_preferences(&app).await {
        Ok(prefs) => {
            info!("📋 Loaded recording preferences: auto_save={}", prefs.auto_save);
            prefs.auto_save
        }
        Err(e) => {
            warn!("Failed to load recording preferences, defaulting to auto_save=true: {}", e);
            true // Default to saving if preferences can't be loaded
        }
    };

    // Always ensure a meeting name is set so incremental saver initializes
    let effective_meeting_name = meeting_name.clone().unwrap_or_else(|| {
        let now = chrono::Local::now();
        format!(
            "Meeting {}",
            now.format("%Y-%m-%d_%H-%M-%S")
        )
    });
    manager.set_meeting_name(Some(effective_meeting_name));

    // Set up error callback
    let app_for_error = app.clone();
    manager.set_error_callback(move |error| {
        let _ = app_for_error.emit("recording-error", error.user_message());
    });

    // Start recording with specified devices and auto_save setting
    let transcription_receiver = manager
        .start_recording(mic_device, system_device, auto_save)
        .await
        .map_err(|e| format!("Failed to start recording: {}", e))?;

    // Store the manager globally to keep it alive
    {
        let mut global_manager = RECORDING_MANAGER.lock().unwrap();
        *global_manager = Some(manager);
    }

    // Set recording flag and reset speech detection flag
    info!("🔍 Setting IS_RECORDING to true and resetting SPEECH_DETECTED_EMITTED");
    IS_RECORDING.store(true, Ordering::SeqCst);
    reset_speech_detected_flag(); // Reset for new recording session

    // Start optimized parallel transcription task and store handle
    let task_handle = transcription::start_transcription_task(app.clone(), transcription_receiver);
    {
        let mut global_task = TRANSCRIPTION_TASK.lock().unwrap();
        *global_task = Some(task_handle);
    }

    // CRITICAL: Listen for transcript-update events and save to recording manager
    // This enables transcript history persistence for page reload sync
    // Store listener ID for cleanup during stop_recording to ensure microphone is released
    {
        use tauri::Listener;
        let listener_id = app.listen("transcript-update", move |event: tauri::Event| {
            // Parse the transcript update from the event payload
            if let Ok(update) = serde_json::from_str::<TranscriptUpdate>(event.payload()) {
                // Create structured transcript segment
                let segment = crate::audio::recording_saver::TranscriptSegment {
                    id: format!("seg_{}", update.sequence_id),
                    text: update.text.clone(),
                    audio_start_time: update.audio_start_time,
                    audio_end_time: update.audio_end_time,
                    duration: update.duration,
                    display_time: update.timestamp.clone(), // Use wall-clock timestamp for display
                    confidence: update.confidence,
                    sequence_id: update.sequence_id,
                    speaker: update.speaker.clone(),
                };

                // Save to recording manager
                if let Ok(manager_guard) = RECORDING_MANAGER.lock() {
                    if let Some(manager) = manager_guard.as_ref() {
                        manager.add_transcript_segment(segment);
                    }
                }
            }
        });
        let mut global_listener = TRANSCRIPT_LISTENER_ID.lock().unwrap();
        *global_listener = Some(listener_id);
        info!("✅ Transcript-update event listener registered for history persistence");
    }

    // Emit success event
    app.emit("recording-started", serde_json::json!({
        "message": "Recording started with custom devices and parallel processing",
        "devices": [
            mic_device_name.unwrap_or_else(|| "Default Microphone".to_string()),
            system_device_name.unwrap_or_else(|| "Default System Audio".to_string())
        ],
        "workers": 3
    })).map_err(|e| e.to_string())?;

    // Update tray menu to reflect recording state
    crate::tray::update_tray_menu(&app);

    info!("✅ Recording started with custom devices using async-first approach");

    Ok(())
}

/// Convert the recording saver's transcript segments into the repository's
/// `TranscriptSegment` shape used by `save_transcript` (the same type the frontend
/// save path deserializes into). The saver type carries a `display_time` label and
/// f64 timings; the repo type wants a `timestamp` string and Option-wrapped timings.
fn recording_segments_to_api(
    segments: &[super::recording_saver::TranscriptSegment],
) -> Vec<crate::api::TranscriptSegment> {
    segments
        .iter()
        .map(|s| crate::api::TranscriptSegment {
            id: s.id.clone(),
            text: s.text.clone(),
            timestamp: s.display_time.clone(),
            audio_start_time: Some(s.audio_start_time),
            audio_end_time: Some(s.audio_end_time),
            duration: Some(s.duration),
            speaker: if s.speaker.is_empty() {
                None
            } else {
                Some(s.speaker.clone())
            },
        })
        .collect()
}

/// Best-effort persist of the in-progress recording's transcript at app exit / tray quit.
/// Does NOT wait for the transcription backlog to drain (that could take minutes) — it
/// saves whatever segments the manager currently holds so a meeting is never lost to a
/// quit. No-op when not recording. Call from an async context (e.g. RunEvent::Exit).
pub async fn save_active_recording_on_exit<R: Runtime>(app: &AppHandle<R>) {
    if !IS_RECORDING.load(Ordering::SeqCst) {
        return;
    }
    info!("💾 App exiting while recording — persisting current transcript before quit");

    // Force-flush the pipeline so audio still buffered (not yet emitted as a
    // transcript-update segment) is pushed into the transcription queue before we
    // snapshot. Take the manager out (never hold the std mutex across .await), flush,
    // then return it so the snapshot below sees the freshly flushed state.
    //
    // HONEST LIMITATION: this guarantees no buffered-but-unflushed audio is silently
    // dropped, but it does NOT wait for the full transcription backlog to drain (that
    // could take minutes and hang exit), and during RunEvent::Exit the transcript-update
    // listener may not pump, so late segments produced by the flush may not append. Full
    // backlog capture at quit comes from routing through stop_recording (tray quit).
    let taken = { RECORDING_MANAGER.lock().unwrap().take() };
    if let Some(mut manager) = taken {
        let _ = manager.stop_streams_and_force_flush().await;
        *RECORDING_MANAGER.lock().unwrap() = Some(manager);
    }

    // Snapshot from the global manager without removing it (sync methods only; the
    // guard is dropped before any await, so no std mutex is held across .await).
    let snapshot = {
        let guard = RECORDING_MANAGER.lock().unwrap();
        guard.as_ref().map(|m| {
            (
                m.get_transcript_segments(),
                m.get_meeting_name(),
                m.get_meeting_folder(),
            )
        })
    };
    let Some((segments, meeting_name, meeting_folder)) = snapshot else {
        return;
    };
    if segments.is_empty() {
        return;
    }
    let Some(state) = app.try_state::<crate::state::AppState>() else {
        warn!("AppState unavailable at exit; cannot persist in-progress recording");
        return;
    };
    let title = meeting_name.unwrap_or_else(|| "New Meeting".to_string());
    let folder_path = meeting_folder.map(|p| p.to_string_lossy().to_string());
    let api_segments = recording_segments_to_api(&segments);
    match crate::database::repositories::transcript::TranscriptsRepository::save_transcript(
        state.db_manager.pool(),
        &title,
        &api_segments,
        folder_path,
    )
    .await
    {
        Ok(id) => info!("💾 Persisted in-progress meeting on exit: id={}", id),
        Err(e) => error!("❌ Failed to persist meeting on exit: {}", e),
    }
}

/// Stop recording with optimized graceful shutdown ensuring NO transcript chunks are lost
pub async fn stop_recording<R: Runtime>(
    app: AppHandle<R>,
    _args: RecordingArgs,
) -> Result<(), String> {
    info!(
        "🛑 Starting optimized recording shutdown - ensuring ALL transcript chunks are preserved"
    );

    // Check if recording is active
    if !IS_RECORDING.load(Ordering::SeqCst) {
        info!("Recording was not active");
        return Ok(());
    }

    // Emit shutdown progress to frontend
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "stopping_audio",
            "message": "Stopping audio capture...",
            "progress": 20
        }),
    );

    // Step 1: Stop audio capture immediately (no more new chunks) with proper error handling.
    // Take the manager only long enough to force-flush the pipeline, then RETURN it to the
    // global so the transcript-update listener (which reaches the manager via the global)
    // keeps appending late segments while the transcription backlog drains in Step 2. We
    // take it back for good in Step 4 to save it — this way get_transcript_segments() there
    // includes the drain-time tail, not just what existed when Stop was pressed.
    let manager_for_flush = {
        let mut global_manager = RECORDING_MANAGER.lock().unwrap();
        global_manager.take()
    };

    let stop_result = if let Some(mut manager) = manager_for_flush {
        // Use FORCE FLUSH to immediately process all accumulated audio - eliminates 30s delay!
        info!("🚀 Using FORCE FLUSH to eliminate pipeline accumulation delays");
        let result = manager.stop_streams_and_force_flush().await;
        match &result {
            // On success, return the manager to the global for the duration of the drain.
            Ok(_) => {
                *RECORDING_MANAGER.lock().unwrap() = Some(manager);
            }
            // On failure we bail out below; drop the manager rather than orphan it.
            Err(_) => {}
        }
        result
    } else {
        warn!("No recording manager found to stop");
        Ok(())
    };

    match stop_result {
        Ok(_) => {
            info!("✅ Audio streams stopped successfully - no more chunks will be created");
        }
        Err(e) => {
            error!("❌ Failed to stop audio streams: {}", e);
            return Err(format!("Failed to stop audio streams: {}", e));
        }
    }

    // NOTE: the transcript-update listener is deliberately kept ALIVE here (it used to be
    // removed at this point). It is unlistened after the Step 2 drain so late segments
    // transcribed during the backlog drain still reach the manager and transcripts.json.

    // Step 2: Signal transcription workers to finish processing ALL queued chunks
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "processing_transcripts",
            "message": "Processing remaining transcript chunks...",
            "progress": 40
        }),
    );

    // Wait for transcription task with enhanced progress monitoring (NO TIMEOUT - we must process all chunks)
    let transcription_task = {
        let mut global_task = TRANSCRIPTION_TASK.lock().unwrap();
        global_task.take()
    };

    if let Some(task_handle) = transcription_task {
        info!("⏳ Waiting for ALL transcription chunks to be processed (no timeout - preserving every chunk)");

        // Enhanced progress monitoring during shutdown
        let progress_app = app.clone();
        let progress_task = tokio::spawn(async move {
            let last_update = std::time::Instant::now();

            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                // Emit periodic progress updates during shutdown
                let elapsed = last_update.elapsed().as_secs();
                let _ = progress_app.emit(
                    "recording-shutdown-progress",
                    serde_json::json!({
                        "stage": "processing_transcripts",
                        "message": format!("Processing transcripts... ({}s elapsed)", elapsed),
                        "progress": 40,
                        "detailed": true,
                        "elapsed_seconds": elapsed
                    }),
                );
            }
        });

        // Wait up to 10 minutes for transcription completion to prevent indefinite hangs
        match tokio::time::timeout(
            tokio::time::Duration::from_secs(600), // 10 minutes max
            task_handle
        ).await {
            Ok(Ok(())) => {
                info!("✅ ALL transcription chunks processed successfully - no data lost");
            }
            Ok(Err(e)) => {
                warn!("⚠️ Transcription task completed with error: {:?}", e);
                // Continue anyway - the worker may have processed most chunks
            }
            Err(_) => {
                warn!("⏱️ Transcription timeout (10 minutes) reached, continuing shutdown to prevent indefinite hang");
                // Continue shutdown even on timeout - better to lose some chunks than hang forever
            }
        }

        // Stop progress monitoring
        progress_task.abort();
    } else {
        info!("ℹ️ No transcription task found to wait for");
    }

    // Step 2.5: Now that the backlog is drained, remove the transcript-update listener.
    // Doing this AFTER the drain (it used to run before) ensures late segments produced
    // while the backlog drained were still appended to the manager and transcripts.json.
    {
        use tauri::Listener;
        if let Some(listener_id) = TRANSCRIPT_LISTENER_ID.lock().unwrap().take() {
            app.unlisten(listener_id);
            info!("✅ Transcript-update listener removed (after drain)");
        }
    }

    // Step 3: Now safely unload Whisper model after ALL chunks are processed
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "unloading_model",
            "message": "Unloading speech recognition model...",
            "progress": 70
        }),
    );

    info!("🧠 All transcript chunks processed. Now safely unloading transcription model...");

    // Determine which provider was used and unload the appropriate model (with timeout)
    let config = match tokio::time::timeout(
        tokio::time::Duration::from_secs(30), // 30 seconds max for DB operation
        crate::api::api::api_get_transcript_config(
            app.clone(),
            app.clone().state(),
            None,
        )
    )
    .await
    {
        Ok(Ok(Some(config))) => Some(config.provider),
        Ok(Ok(None)) => None,
        Ok(Err(e)) => {
            warn!("⚠️ Failed to get transcript config: {:?}", e);
            None
        }
        Err(_) => {
            warn!("⏱️ Transcript config timeout (30s), continuing shutdown");
            None
        }
    };

    match config.as_deref() {
        Some("parakeet") => {
            info!("🦜 Unloading Parakeet model...");
            let engine_clone = {
                let engine_guard = crate::parakeet_engine::commands::PARAKEET_ENGINE
                    .lock()
                    .unwrap();
                engine_guard.as_ref().cloned()
            };

            if let Some(engine) = engine_clone {
                let current_model = engine
                    .get_current_model()
                    .await
                    .unwrap_or_else(|| "unknown".to_string());
                info!("Current Parakeet model before unload: '{}'", current_model);

                if engine.unload_model().await {
                    info!("✅ Parakeet model '{}' unloaded successfully", current_model);
                } else {
                    warn!("⚠️ Failed to unload Parakeet model '{}'", current_model);
                }
            } else {
                warn!("⚠️ No Parakeet engine found to unload model");
            }
        }
        _ => {
            // Default to Whisper
            info!("🎤 Unloading Whisper model...");
            let engine_clone = {
                let engine_guard = crate::whisper_engine::commands::WHISPER_ENGINE
                    .lock()
                    .unwrap();
                engine_guard.as_ref().cloned()
            };

            if let Some(engine) = engine_clone {
                let current_model = engine
                    .get_current_model()
                    .await
                    .unwrap_or_else(|| "unknown".to_string());
                info!("Current Whisper model before unload: '{}'", current_model);

                if engine.unload_model().await {
                    info!("✅ Whisper model '{}' unloaded successfully", current_model);
                } else {
                    warn!("⚠️ Failed to unload Whisper model '{}'", current_model);
                }
            } else {
                warn!("⚠️ No Whisper engine found to unload model");
            }
        }
    }

    // Step 4: Finalize recording state and cleanup resources safely
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "finalizing",
            "message": "Finalizing recording and cleaning up resources...",
            "progress": 90
        }),
    );

    // Take the manager back from the global for good and perform final cleanup + save.
    let manager_for_cleanup = {
        let mut global_manager = RECORDING_MANAGER.lock().unwrap();
        global_manager.take()
    };

    // meeting_id of the row Rust persisted, if any — sent to the frontend so it navigates
    // to the saved meeting instead of creating a duplicate.
    let mut saved_meeting_id: Option<String> = None;

    // Perform final cleanup with the manager if available
    let (meeting_folder, meeting_name) = if let Some(mut manager) = manager_for_cleanup {
        info!("🧹 Performing final cleanup and saving recording data");

        // Extract meeting info BEFORE async operations
        let meeting_folder = manager.get_meeting_folder();
        let meeting_name = manager.get_meeting_name();

        match tokio::time::timeout(
            tokio::time::Duration::from_secs(300), // 5 minutes max for file I/O
            manager.save_recording_only(&app)
        ).await {
            Ok(Ok(_)) => {
                info!("✅ Recording data saved successfully during cleanup");
            }
            Ok(Err(e)) => {
                warn!(
                    "⚠️ Error during recording cleanup (transcripts preserved): {}",
                    e
                );
                // Don't fail shutdown - transcripts are already preserved
            }
            Err(_) => {
                warn!("⏱️ File I/O timeout (5 minutes) reached during save, continuing shutdown");
                // Don't fail shutdown - transcripts are already preserved
            }
        }

        // Step 4.1: Rust is the WRITER OF RECORD. Persist the meeting to SQLite now, so
        // a meeting can never be lost by a webview error/reload/quit or the fragile
        // frontend save gate. Reuses the same repository the frontend save path uses;
        // the returned meeting_id is handed to the frontend so it does not save again.
        let segments = manager.get_transcript_segments();
        if segments.is_empty() {
            info!("ℹ️ No transcript segments to persist at stop");
        } else if let Some(state) = app.try_state::<crate::state::AppState>() {
            let title = meeting_name
                .clone()
                .unwrap_or_else(|| "New Meeting".to_string());
            let folder_path = meeting_folder
                .as_ref()
                .map(|p| p.to_string_lossy().to_string());
            let api_segments = recording_segments_to_api(&segments);
            match crate::database::repositories::transcript::TranscriptsRepository::save_transcript(
                state.db_manager.pool(),
                &title,
                &api_segments,
                folder_path,
            )
            .await
            {
                Ok(id) => {
                    info!(
                        "💾 Saved meeting to DB (writer of record) id={} with {} segments",
                        id,
                        api_segments.len()
                    );
                    saved_meeting_id = Some(id);
                }
                Err(e) => {
                    // Non-fatal: the frontend save path remains as a fallback.
                    error!("❌ Failed to save meeting to DB at stop: {} (frontend will fall back)", e);
                }
            }
        } else {
            warn!("⚠️ AppState/database not available at stop; frontend will save");
        }

        (meeting_folder, meeting_name)
    } else {
        info!("ℹ️ No recording manager available for cleanup");
        (None, None)
    };

    // Set recording flag to false
    info!("🔍 Setting IS_RECORDING to false");
    IS_RECORDING.store(false, Ordering::SeqCst);

    // Step 4.5: Prepare metadata for the frontend. The DB row is already written above
    // (saved_meeting_id); the frontend uses these to refresh/navigate, and only falls
    // back to saving itself if no meeting_id arrived.
    let (folder_path_str, meeting_name_str) = match (&meeting_folder, &meeting_name) {
        (Some(path), Some(name)) => (
            Some(path.to_string_lossy().to_string()),
            Some(name.clone()),
        ),
        _ => (None, None),
    };

    info!("📤 Preparing recording metadata for frontend");
    info!("   folder_path: {:?}", folder_path_str);
    info!("   meeting_name: {:?}", meeting_name_str);
    info!("   saved_meeting_id: {:?}", saved_meeting_id);

    // Step 5: Complete shutdown
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "complete",
            "message": "Recording stopped successfully",
            "progress": 100
        }),
    );

    // Emit final stop event. meeting_id is the row Rust already persisted; when present
    // the frontend navigates to it and skips its own save (avoids a duplicate meeting).
    app.emit(
        "recording-stopped",
        serde_json::json!({
            "message": "Recording stopped",
            "folder_path": folder_path_str,
            "meeting_name": meeting_name_str,
            "meeting_id": saved_meeting_id
        }),
    )
    .map_err(|e| e.to_string())?;

    // Update tray menu to reflect stopped state
    crate::tray::update_tray_menu(&app);

    info!("🎉 Recording stopped successfully with ZERO transcript chunks lost");
    Ok(())
}

/// Check if recording is active
pub async fn is_recording() -> bool {
    IS_RECORDING.load(Ordering::SeqCst)
}

/// Live transcription status. Reports a REAL in-flight backlog (queued minus completed,
/// published by the transcription worker) so the post-stop wait loop actually waits for
/// the queue to drain instead of exiting immediately on a hardcoded value.
#[tauri::command]
pub async fn get_transcription_status() -> TranscriptionStatus {
    let is_recording = IS_RECORDING.load(Ordering::SeqCst);
    let (chunks_in_queue, is_processing, last_activity_ms) =
        crate::audio::transcription::worker::transcription_progress(is_recording);
    TranscriptionStatus {
        chunks_in_queue,
        is_processing,
        last_activity_ms,
    }
}

/// Pause the current recording
#[tauri::command]
pub async fn pause_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    info!("Pausing recording");

    // Check if currently recording
    if !IS_RECORDING.load(Ordering::SeqCst) {
        return Err("No recording is currently active".to_string());
    }

    // Access the recording manager and pause it
    let manager_guard = RECORDING_MANAGER.lock().unwrap();
    if let Some(manager) = manager_guard.as_ref() {
        manager.pause_recording().map_err(|e| e.to_string())?;

        // Emit pause event to frontend
        app.emit(
            "recording-paused",
            serde_json::json!({
                "message": "Recording paused"
            }),
        )
        .map_err(|e| e.to_string())?;

        // Update tray menu to reflect paused state
        crate::tray::update_tray_menu(&app);

        info!("Recording paused successfully");
        Ok(())
    } else {
        Err("No recording manager found".to_string())
    }
}

/// Resume the current recording
#[tauri::command]
pub async fn resume_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    info!("Resuming recording");

    // Check if currently recording
    if !IS_RECORDING.load(Ordering::SeqCst) {
        return Err("No recording is currently active".to_string());
    }

    // Access the recording manager and resume it
    let manager_guard = RECORDING_MANAGER.lock().unwrap();
    if let Some(manager) = manager_guard.as_ref() {
        manager.resume_recording().map_err(|e| e.to_string())?;

        // Emit resume event to frontend
        app.emit(
            "recording-resumed",
            serde_json::json!({
                "message": "Recording resumed"
            }),
        )
        .map_err(|e| e.to_string())?;

        // Update tray menu to reflect resumed state
        crate::tray::update_tray_menu(&app);

        info!("Recording resumed successfully");
        Ok(())
    } else {
        Err("No recording manager found".to_string())
    }
}

/// Check if recording is currently paused
#[tauri::command]
pub async fn is_recording_paused() -> bool {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();
    if let Some(manager) = manager_guard.as_ref() {
        manager.is_paused()
    } else {
        false
    }
}

/// Get detailed recording state
#[tauri::command]
pub async fn get_recording_state() -> serde_json::Value {
    let is_recording = IS_RECORDING.load(Ordering::SeqCst);
    let manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_ref() {
        serde_json::json!({
            "is_recording": is_recording,
            "is_paused": manager.is_paused(),
            "is_active": manager.is_active(),
            "recording_duration": manager.get_recording_duration(),
            "active_duration": manager.get_active_recording_duration(),
            "total_pause_duration": manager.get_total_pause_duration(),
            "current_pause_duration": manager.get_current_pause_duration()
        })
    } else {
        serde_json::json!({
            "is_recording": is_recording,
            "is_paused": false,
            "is_active": false,
            "recording_duration": null,
            "active_duration": null,
            "total_pause_duration": 0.0,
            "current_pause_duration": null
        })
    }
}

/// Get the meeting folder path for the current recording
/// Returns the path if a meeting name was set and folder structure initialized
#[tauri::command]
pub async fn get_meeting_folder_path() -> Result<Option<String>, String> {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();
    if let Some(manager) = manager_guard.as_ref() {
        Ok(manager.get_meeting_folder().map(|p| p.to_string_lossy().to_string()))
    } else {
        Ok(None)
    }
}

/// Get accumulated transcript segments from current recording session
/// Used for syncing frontend state after page reload during active recording
#[tauri::command]
pub async fn get_transcript_history() -> Result<Vec<crate::audio::recording_saver::TranscriptSegment>, String> {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_ref() {
        Ok(manager.get_transcript_segments())
    } else {
        Ok(Vec::new()) // No recording active, return empty
    }
}

/// Get meeting name from current recording session
/// Used for syncing frontend state after page reload during active recording
#[tauri::command]
pub async fn get_recording_meeting_name() -> Result<Option<String>, String> {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_ref() {
        Ok(manager.get_meeting_name())
    } else {
        Ok(None)
    }
}

// ============================================================================
// DEVICE MONITORING COMMANDS (AirPods/Bluetooth disconnect/reconnect support)
// ============================================================================

/// Response structure for device events
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type")]
pub enum DeviceEventResponse {
    DeviceDisconnected {
        device_name: String,
        device_type: String,
    },
    DeviceReconnected {
        device_name: String,
        device_type: String,
    },
    DeviceListChanged,
}

impl From<DeviceEvent> for DeviceEventResponse {
    fn from(event: DeviceEvent) -> Self {
        match event {
            DeviceEvent::DeviceDisconnected { device_name, device_type } => {
                DeviceEventResponse::DeviceDisconnected {
                    device_name,
                    device_type: format!("{:?}", device_type),
                }
            }
            DeviceEvent::DeviceReconnected { device_name, device_type } => {
                DeviceEventResponse::DeviceReconnected {
                    device_name,
                    device_type: format!("{:?}", device_type),
                }
            }
            DeviceEvent::DeviceListChanged => DeviceEventResponse::DeviceListChanged,
        }
    }
}

/// Reconnection status information
#[derive(Debug, Serialize, Clone)]
pub struct ReconnectionStatus {
    pub is_reconnecting: bool,
    pub disconnected_device: Option<DisconnectedDeviceInfo>,
}

/// Information about a disconnected device
#[derive(Debug, Serialize, Clone)]
pub struct DisconnectedDeviceInfo {
    pub name: String,
    pub device_type: String,
}

/// Poll for audio device events (disconnect/reconnect)
/// Should be called periodically (every 1-2 seconds) by frontend during recording
#[tauri::command]
pub async fn poll_audio_device_events() -> Result<Option<DeviceEventResponse>, String> {
    let mut manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_mut() {
        if let Some(event) = manager.poll_device_events() {
            info!("📱 Device event polled: {:?}", event);
            Ok(Some(event.into()))
        } else {
            Ok(None)
        }
    } else {
        // Not recording, no events
        Ok(None)
    }
}

/// Get current reconnection status
/// Returns whether the system is attempting to reconnect and which device
#[tauri::command]
pub async fn get_reconnection_status() -> Result<ReconnectionStatus, String> {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_ref() {
        let state = manager.get_state();
        let disconnected_device = state.get_disconnected_device().map(|(device, device_type)| {
            DisconnectedDeviceInfo {
                name: device.name.clone(),
                device_type: format!("{:?}", device_type),
            }
        });

        Ok(ReconnectionStatus {
            is_reconnecting: manager.is_reconnecting(),
            disconnected_device,
        })
    } else {
        // Not recording, no reconnection in progress
        Ok(ReconnectionStatus {
            is_reconnecting: false,
            disconnected_device: None,
        })
    }
}

/// Get information about the active audio output device
/// Used to warn users about Bluetooth playback issues
#[tauri::command]
pub async fn get_active_audio_output() -> Result<super::playback_monitor::AudioOutputInfo, String> {
    super::playback_monitor::get_active_audio_output()
        .await
        .map_err(|e| format!("Failed to get audio output info: {}", e))
}

/// Manually trigger device reconnection attempt
/// Useful for UI "Retry" button
#[tauri::command]
pub async fn attempt_device_reconnect(
    device_name: String,
    device_type: String,
) -> Result<bool, String> {
    // Parse device type first
    let monitor_type = match device_type.as_str() {
        "Microphone" => DeviceMonitorType::Microphone,
        "SystemAudio" => DeviceMonitorType::SystemAudio,
        _ => return Err(format!("Invalid device type: {}", device_type)),
    };

    // Check if recording is active
    {
        let manager_guard = RECORDING_MANAGER.lock().unwrap();
        if manager_guard.is_none() {
            return Err("Recording not active".to_string());
        }
    } // Release lock

    // Spawn blocking task to handle the async reconnection
    let result = tokio::task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(async {
            let mut manager_guard = RECORDING_MANAGER.lock().unwrap();
            if let Some(manager) = manager_guard.as_mut() {
                manager.attempt_device_reconnect(&device_name, monitor_type).await
            } else {
                Err(anyhow::anyhow!("Recording not active"))
            }
        })
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?;

    match result {
        Ok(success) => {
            if success {
                info!("✅ Manual reconnection successful");
            } else {
                warn!("❌ Manual reconnection failed - device not available");
            }
            Ok(success)
        }
        Err(e) => {
            error!("Manual reconnection error: {}", e);
            Err(e.to_string())
        }
    }
}
