# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Meetily** is a privacy-first AI meeting assistant that captures, transcribes, and summarizes meetings entirely on local infrastructure. It is a single Tauri desktop application — everything (audio capture, transcription, storage, summarization, chat) runs in-process:

- **Desktop App**: Tauri 2.x (Rust + Next.js 14 + React 18 + TypeScript)
- **Audio Processing**: Rust (cpal, WASAPI/ScreenCaptureKit, professional audio mixing)
- **Transcription**: Whisper.cpp and Parakeet (local, GPU-accelerated, in-process)
- **Speaker Diarization**: Rust-native (sherpa-onnx), `src-tauri/src/diarization/`
- **Persistence**: Local SQLite via sqlx (`src-tauri/src/database/`, migrations in `src-tauri/migrations/`)
- **LLM Integration**: Ollama, LM Studio, Claude, OpenAI, Groq, OpenRouter, custom OpenAI-compatible, plus a bundled llama.cpp sidecar (`llama-helper/`, root workspace member)
- **MCP Server** (optional tooling): `backend/mcp_server/` — read-only MCP access to the app's SQLite database. It does not run a backend; it opens the DB file directly (`DATABASE_PATH` env var).

> **History note**: the repo once contained a Python FastAPI backend (port 5167) and an external whisper-server (port 8178). Both were removed — every feature they provided lives in the Rust side now. If you find references to them in docs or comments, they are stale.

## Essential Development Commands

**Location**: `/frontend`

```bash
# macOS Development
./clean_run.sh              # Clean build and run with info logging
./clean_run.sh debug        # Run with debug logging
./clean_build.sh            # Production build

# Windows Development
clean_run_windows.bat       # Clean build and run
clean_build_windows.bat     # Production build

# Manual Commands
pnpm install                # Install dependencies
pnpm run dev                # Next.js dev server (port 3118)
pnpm run tauri:dev          # Full Tauri development mode
pnpm run tauri:build        # Production build

# GPU-Specific Builds (for testing acceleration)
pnpm run tauri:dev:metal    # macOS Metal GPU
pnpm run tauri:dev:cuda     # NVIDIA CUDA
pnpm run tauri:dev:vulkan   # AMD/Intel Vulkan
pnpm run tauri:dev:cpu      # CPU-only (no GPU)
```

Rust workspace commands run from the repo root (workspace members: `frontend/src-tauri`, `llama-helper`):

```bash
cargo check --workspace
```

**Available Whisper Models**: `tiny`, `tiny.en`, `base`, `base.en`, `small`, `small.en`, `medium`, `medium.en`, `large-v1`, `large-v2`, `large-v3`, `large-v3-turbo`

## High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Tauri Desktop App                            │
│  ┌──────────────────┐  ┌─────────────────┐  ┌────────────────┐ │
│  │   Next.js UI     │  │  Rust Backend   │  │  STT Engines   │ │
│  │  (React/TS)      │←→│  (Audio + IPC)  │←→│ Whisper/Parakeet│ │
│  └──────────────────┘  └────────┬────────┘  └────────────────┘ │
│    ↑ Tauri events / invoke      │                               │
│                        ┌────────┴────────┐  ┌────────────────┐ │
│                        │  SQLite (sqlx)  │  │  LLM Clients   │ │
│                        │ meetings, chat, │  │ Ollama/Claude/ │ │
│                        │ summaries, cfg  │  │ llama sidecar  │ │
│                        └─────────────────┘  └────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

All data access from the UI goes through Tauri `invoke()` → Rust commands → repository structs over a shared SQLite pool. There is no HTTP API.

### Audio Processing Pipeline (Critical Understanding)

The audio system has **two parallel paths** with different purposes:

```
Raw Audio (Mic + System)
         ↓
┌────────────────────────────────────────────────────────────┐
│              Audio Pipeline Manager                         │
│  (frontend/src-tauri/src/audio/pipeline.rs)                │
└─────────────┬──────────────────────────┬───────────────────┘
              ↓                          ↓
    ┌─────────────────┐        ┌─────────────────────┐
    │ Recording Path  │        │ Transcription Path  │
    │ (Pre-mixed)     │        │ (VAD-filtered)      │
    └─────────────────┘        └─────────────────────┘
              ↓                          ↓
    RecordingSaver.save()      WhisperEngine.transcribe()
```

**Key Insight**: The pipeline performs **professional audio mixing** (RMS-based ducking, clipping prevention) for recording, while simultaneously applying **Voice Activity Detection (VAD)** to send only speech segments to Whisper for transcription.

### Audio Module Layout

```
audio/
├── devices/                    # Device discovery and configuration
│   ├── discovery.rs           # list_audio_devices, trigger_audio_permission
│   ├── microphone.rs          # default_input_device
│   ├── speakers.rs            # default_output_device
│   ├── configuration.rs       # AudioDevice types, parsing
│   └── platform/              # windows.rs (WASAPI), macos.rs (ScreenCaptureKit), linux.rs
├── capture/                   # Audio stream capture (microphone.rs, system.rs, core_audio.rs)
├── pipeline.rs                # Audio mixing and VAD processing
├── recording_manager.rs       # High-level recording coordination
├── recording_commands.rs      # Tauri command interface
├── recording_saver.rs         # Audio file writing
└── retranscription.rs         # Import audio / re-run transcription on saved meetings
```

**When working on audio features**:
- Device detection issues → `devices/discovery.rs` or `devices/platform/{windows,macos,linux}.rs`
- Microphone/speaker problems → `devices/microphone.rs` or `devices/speakers.rs`
- Audio capture issues → `capture/microphone.rs` or `capture/system.rs`
- Mixing/processing problems → `pipeline.rs`
- Recording workflow → `recording_manager.rs`

### Rust ↔ Frontend Communication (Tauri Architecture)

**Command Pattern** (Frontend → Rust):
```typescript
// Frontend: src/app/page.tsx
await invoke('start_recording', {
  mic_device_name: "Built-in Microphone",
  system_device_name: "BlackHole 2ch",
  meeting_name: "Team Standup"
});
```

```rust
// Rust: src/lib.rs
#[tauri::command]
async fn start_recording<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
    meeting_name: Option<String>
) -> Result<(), String> {
    // Implementation delegates to audio::recording_commands
}
```

**Event Pattern** (Rust → Frontend):
```rust
// Rust: Emit transcript updates
app.emit("transcript-update", TranscriptUpdate { /* ... */ })?;
```

```typescript
// Frontend: Listen for events
await listen<TranscriptUpdate>('transcript-update', (event) => {
  setTranscripts(prev => [...prev, event.payload]);
});
```

### Database Layer

**Location**: `frontend/src-tauri/src/database/`

- `manager.rs` — owns the sqlx SQLite pool, runs migrations from `frontend/src-tauri/migrations/`
- `repositories/` — one repository per aggregate: `meeting.rs`, `transcript.rs`, `transcript_chunk.rs`, `summary.rs`, `setting.rs`, `chat.rs`
- Tauri commands in `src/api/api.rs` and `src/api/chat_api.rs` call repositories via `state.db_manager.pool()`

**Adding a schema change**: add a timestamped migration file under `frontend/src-tauri/migrations/` (e.g. `20260707120000_description.sql`); it runs automatically at startup.

### Whisper Model Management

**Model Storage Locations**:
- **Development**: `frontend/models/`
- **Production (macOS)**: `~/Library/Application Support/Meetily/models/`
- **Production (Windows)**: `%APPDATA%\Meetily\models\`

**Model Loading** (frontend/src-tauri/src/whisper_engine/whisper_engine.rs):
```rust
pub async fn load_model(&self, model_name: &str) -> Result<()> {
    // Automatically detects GPU capabilities (Metal/CUDA/Vulkan)
    // Falls back to CPU if GPU unavailable
}
```

**GPU Acceleration**:
- **macOS**: Metal + CoreML (automatically enabled)
- **Windows/Linux**: CUDA (NVIDIA), Vulkan (AMD/Intel), or CPU
- Configure via Cargo features: `--features cuda`, `--features vulkan`
- ⚠️ Do NOT enable CUDA and Vulkan together — a ggml built with both backends crashes on transcription (see git history around v0.5.0)

## Critical Development Patterns

### 1. Audio Buffer Management

**Ring Buffer Mixing** (pipeline.rs):
- Mic and system audio arrive asynchronously at different rates
- Ring buffer accumulates samples until both streams have aligned windows (50ms)
- Professional mixing applies RMS-based ducking to prevent system audio from drowning out microphone
- Uses `VecDeque` for efficient windowed processing

### 2. Thread Safety and Async Boundaries

**Recording State** (recording_state.rs):
```rust
pub struct RecordingState {
    is_recording: Arc<AtomicBool>,
    audio_sender: Arc<RwLock<Option<mpsc::UnboundedSender<AudioChunk>>>>,
    // ...
}
```

**Key Pattern**: Use `Arc<RwLock<T>>` for shared state across async tasks, `Arc<AtomicBool>` for simple flags.

### 3. Error Handling and Logging

**Performance-Aware Logging** (lib.rs):
```rust
#[cfg(debug_assertions)]
macro_rules! perf_debug {
    ($($arg:tt)*) => { log::debug!($($arg)*) };
}

#[cfg(not(debug_assertions))]
macro_rules! perf_debug {
    ($($arg:tt)*) => {};  // Zero overhead in release builds
}
```

**Usage**: Use `perf_debug!()` and `perf_trace!()` for hot-path logging that should be eliminated in production.

### 4. Frontend State Management

**Sidebar Context** (components/Sidebar/SidebarProvider.tsx):
- Global state for meetings list, current meeting, recording status
- Loads data through Tauri `invoke()` commands (no HTTP)

**Pattern**: Tauri commands update Rust state → Emit events → Frontend listeners update React state → Context propagates to components

## Common Development Tasks

### Adding a New Tauri Command

1. Define command in the relevant module (e.g. `src/api/api.rs`):
   ```rust
   #[tauri::command]
   async fn my_command(arg: String) -> Result<String, String> { /* ... */ }
   ```
2. Register in `tauri::Builder` in `src/lib.rs`:
   ```rust
   .invoke_handler(tauri::generate_handler![
       start_recording,
       my_command,  // Add here
   ])
   ```
3. Call from frontend:
   ```typescript
   const result = await invoke<string>('my_command', { arg: 'value' });
   ```

### Modifying Audio Pipeline Behavior

**Location**: `frontend/src-tauri/src/audio/pipeline.rs`

Key components:
- `AudioMixerRingBuffer`: Manages mic + system audio synchronization
- `ProfessionalAudioMixer`: RMS-based ducking and mixing
- `AudioPipelineManager`: Orchestrates VAD, mixing, and distribution

**Testing Audio Changes**:
```bash
# Enable verbose audio logging
RUST_LOG=app_lib::audio=debug ./clean_run.sh
```

## Testing and Debugging

**Enable Rust Logging**:
```bash
# macOS
RUST_LOG=debug ./clean_run.sh

# Windows (PowerShell)
$env:RUST_LOG="debug"; ./clean_run_windows.bat
```

**Developer Tools**:
- Open DevTools: `Cmd+Shift+I` (macOS) or `Ctrl+Shift+I` (Windows)
- Console Toggle: Built into app UI (console icon)
- View Rust logs: Check terminal output

**Audio Pipeline Metrics** (emitted by pipeline): buffer sizes, mixing window count, VAD detection rate, dropped chunk warnings — visible in the in-app developer console while recording.

## Platform-Specific Notes

### macOS
- **Audio Capture**: Uses ScreenCaptureKit for system audio (macOS 13+)
- **GPU**: Metal + CoreML automatically enabled
- **Permissions**: Requires microphone + screen recording permissions

### Windows
- **Audio Capture**: Uses WASAPI; system audio via WASAPI loopback
- **GPU**: CUDA (NVIDIA) or Vulkan (AMD/Intel) via Cargo features
- **Build Tools**: Requires Visual Studio Build Tools with C++ workload

### Linux
- **Audio Capture**: ALSA/PulseAudio
- **GPU**: CUDA (NVIDIA) or Vulkan via Cargo features
- **Dependencies**: Requires cmake, llvm, libomp

## Performance Optimization Guidelines

- Use `perf_debug!()` / `perf_trace!()` for hot-path logging (zero cost in release)
- Batch audio metrics using `AudioMetricsBatcher` (pipeline.rs)
- Pre-allocate buffers with `AudioBufferPool` (buffer_pool.rs)
- VAD filtering reduces Whisper load by ~70% (only processes speech)
- **Model Selection**: `base`/`small` for dev iteration, `medium`/`large-v3` for quality; GPU is 5-10x faster than CPU

## Important Constraints and Gotchas

1. **Audio Chunk Size**: Pipeline expects consistent 48kHz sample rate. Resampling happens at capture time.
2. **Platform Audio Quirks**: macOS ScreenCaptureKit requires macOS 13+ and screen recording permission; WASAPI exclusive mode can conflict with other apps.
3. **Whisper Model Loading**: Models are loaded once and cached. Changing models requires app restart or manual unload/reload.
4. **Diarization**: sherpa-onnx / ONNX Runtime aborts can crash the whole app — treat diarization changes with care (see git history: auto-diarization after recording was removed for this reason).
5. **File Paths**: Use Tauri's path APIs (`downloadDir`, etc.) for cross-platform compatibility. Never hardcode paths.
6. **Audio Permissions**: Request permissions early. macOS requires both microphone AND screen recording for system audio.

## Repository-Specific Conventions

- **Error Handling**: Rust uses `anyhow::Result`, frontend uses try-catch with user-friendly messages
- **Naming**: Audio devices use "microphone" and "system" consistently (not "input"/"output")
- **Exports**: No index/barrel files — export from the defining file and import from it directly
- **Git Branches**: `main` is the single long-lived branch; short-lived `fix/*` / `feat/*` branches merge back into it

## Key Files Reference

**Core Coordination**:
- [frontend/src-tauri/src/lib.rs](frontend/src-tauri/src/lib.rs) - Main Tauri entry point, command registration
- [frontend/src-tauri/src/audio/mod.rs](frontend/src-tauri/src/audio/mod.rs) - Audio module exports

**Audio System**:
- [frontend/src-tauri/src/audio/recording_manager.rs](frontend/src-tauri/src/audio/recording_manager.rs) - Recording orchestration
- [frontend/src-tauri/src/audio/pipeline.rs](frontend/src-tauri/src/audio/pipeline.rs) - Audio mixing and VAD
- [frontend/src-tauri/src/audio/recording_saver.rs](frontend/src-tauri/src/audio/recording_saver.rs) - Audio file writing

**Data & Commands**:
- [frontend/src-tauri/src/database/manager.rs](frontend/src-tauri/src/database/manager.rs) - SQLite pool + migrations
- [frontend/src-tauri/src/api/api.rs](frontend/src-tauri/src/api/api.rs) - Meeting/transcript/settings commands
- [frontend/src-tauri/src/api/chat_api.rs](frontend/src-tauri/src/api/chat_api.rs) - Per-meeting chat commands

**Transcription & Diarization**:
- [frontend/src-tauri/src/whisper_engine/whisper_engine.rs](frontend/src-tauri/src/whisper_engine/whisper_engine.rs) - Whisper model management and transcription
- [frontend/src-tauri/src/diarization/engine.rs](frontend/src-tauri/src/diarization/engine.rs) - Speaker diarization

**UI Components**:
- [frontend/src/app/page.tsx](frontend/src/app/page.tsx) - Main recording interface
- [frontend/src/components/Sidebar/SidebarProvider.tsx](frontend/src/components/Sidebar/SidebarProvider.tsx) - Global state management
