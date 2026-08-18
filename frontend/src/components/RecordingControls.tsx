'use client';

import { invoke } from '@tauri-apps/api/core';
import { appDataDir } from '@tauri-apps/api/path';
import { useCallback, useEffect, useState, useRef } from 'react';
import { motion } from 'framer-motion';
import { Play, Pause, Square, Mic, MicOff, AlertCircle, X } from 'lucide-react';
import { ProcessRequest, SummaryResponse } from '@/types/summary';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';

interface RecordingControlsProps {
  isRecording: boolean;
  barHeights: string[];
  onRecordingStop: (callApi?: boolean) => void;
  onRecordingStart: () => void;
  onTranscriptReceived: (summary: SummaryResponse) => void;
  onTranscriptionError?: (message: string) => void;
  onStopInitiated?: () => void; // Called immediately when stop button is clicked
  isRecordingDisabled: boolean;
  isParentProcessing: boolean;
  selectedDevices?: {
    micDevice: string | null;
    systemDevice: string | null;
  };
  meetingName?: string;
}

export const RecordingControls: React.FC<RecordingControlsProps> = ({
  isRecording,
  barHeights,
  onRecordingStop,
  onRecordingStart,
  onTranscriptReceived,
  onTranscriptionError,
  onStopInitiated,
  isRecordingDisabled,
  isParentProcessing,
  selectedDevices,
  meetingName,
}) => {
  // Use global recording state context for pause state (syncs with tray operations)
  const recordingState = useRecordingState();
  const isPaused = recordingState.isPaused;
  const isMicMuted = recordingState.isMicMuted;

  const [showPlayback, setShowPlayback] = useState(false);
  const [recordingPath, setRecordingPath] = useState<string | null>(null);
  const [transcript, setTranscript] = useState<string>('');
  const [isProcessing, setIsProcessing] = useState(false);
  const [isStarting, setIsStarting] = useState(false);
  const [isStopping, setIsStopping] = useState(false);
  const [isPausing, setIsPausing] = useState(false);
  const [isResuming, setIsResuming] = useState(false);
  const [isTogglingMute, setIsTogglingMute] = useState(false);
  const MIN_RECORDING_DURATION = 2000; // 2 seconds minimum recording time
  const [transcriptionErrors, setTranscriptionErrors] = useState(0);
  const [isValidatingModel, setIsValidatingModel] = useState(false);
  const [speechDetected, setSpeechDetected] = useState(false);
  const [deviceError, setDeviceError] = useState<{ title: string, message: string } | null>(null);
  // A dead device makes the backend report recoverable errors continuously; throttle the
  // toast so it can't flood sonner (which triggers "Maximum update depth exceeded").
  const lastRecordingErrorToastRef = useRef(0);

  const currentTime = 0;
  const duration = 0;
  const isPlaying = false;
  const progress = 0;

  const formatTime = (time: number) => {
    const minutes = Math.floor(time / 60);
    const seconds = Math.floor(time % 60);
    return `${minutes}:${seconds.toString().padStart(2, '0')}`;
  };

  useEffect(() => {
    const checkTauri = async () => {
      try {
        const result = await invoke('is_recording');
        console.log('Tauri is initialized and ready, is_recording result:', result);
      } catch (error) {
        console.error('Tauri initialization error:', error);
        toast.error('Failed to initialize recording. Please check the console for details.');
      }
    };
    checkTauri();
  }, []);

  const handleStartRecording = useCallback(async () => {
    if (isStarting || isValidatingModel) return;
    console.log('Starting recording...');
    console.log('Selected devices:', selectedDevices);
    console.log('Meeting name:', meetingName);
    console.log('Current isRecording state:', isRecording);

    setShowPlayback(false);
    setTranscript(''); // Clear any previous transcript
    setSpeechDetected(false); // Reset speech detection on new recording

    // Instant feedback: flip the button into its spinner/disabled state the moment
    // it's clicked. Without this the (potentially multi-second) model check + backend
    // start ran with the button looking idle — reading as a freeze.
    setIsStarting(true);
    try {
      // Call the validation callback which will:
      // 1. Check if model is ready
      // 2. Show appropriate toast/modal
      // 3. Call backend if valid
      // 4. Update UI state
      await onRecordingStart();
    } catch (error) {
      console.error('Failed to start recording:', error);
      console.error('Error details:', {
        message: error instanceof Error ? error.message : String(error),
        name: error instanceof Error ? error.name : 'Unknown',
        stack: error instanceof Error ? error.stack : undefined
      });

      // Parse error message to provide user-friendly feedback
      const errorMsg = error instanceof Error ? error.message : String(error);

      // Check for device-related errors
      if (errorMsg.includes('microphone') || errorMsg.includes('mic') || errorMsg.includes('input')) {
        setDeviceError({
          title: 'Microphone Not Available',
          message: 'Unable to access your microphone. Please check that:\n• Your microphone is connected\n• The app has microphone permissions\n• No other app is using the microphone'
        });
      } else if (errorMsg.includes('system audio') || errorMsg.includes('speaker') || errorMsg.includes('output')) {
        setDeviceError({
          title: 'System Audio Not Available',
          message: 'Unable to capture system audio. Please check that:\n• A virtual audio device (like BlackHole) is installed\n• The app has screen recording permissions (macOS)\n• System audio is properly configured'
        });
      } else if (errorMsg.includes('permission')) {
        setDeviceError({
          title: 'Permission Required',
          message: 'Recording permissions are required. Please:\n• Grant microphone access in System Settings\n• Grant screen recording access for system audio (macOS)\n• Restart the app after granting permissions'
        });
      } else {
        setDeviceError({
          title: 'Recording Failed',
          message: 'Unable to start recording. Please check your audio device settings and try again.'
        });
      }
    } finally {
      // RecordingStateContext drives the RECORDING state on success; either way,
      // release the local "starting" spinner.
      setIsStarting(false);
    }
  }, [onRecordingStart, isStarting, isValidatingModel, selectedDevices, meetingName, isRecording]);

  const stopRecordingAction = useCallback(async () => {
    console.log('Executing stop recording...');
    try {
      setIsProcessing(true);
      const dataDir = await appDataDir();
      const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
      const savePath = `${dataDir}/recording-${timestamp}.wav`;
      console.log('Saving recording to:', savePath);
      console.log('About to call stop_recording command');
      const result = await invoke('stop_recording', {
        args: {
          save_path: savePath
        }
      });
      console.log('stop_recording command completed successfully:', result);
      setRecordingPath(savePath);
      // setShowPlayback(true);
      setIsProcessing(false);
      onRecordingStop(true);
    } catch (error) {
      console.error('Failed to stop recording:', error);
      setIsProcessing(false);
      const errorText = error instanceof Error ? error.message : String(error);
      if (errorText.includes('No recording in progress')) {
        return;
      }
      // The stop invoke failed. Only run the post-stop save flow if the backend
      // actually stopped: running it against a still-live recording saves a partial
      // duplicate meeting, wipes the live transcript view, and navigates away while
      // audio keeps recording — with no way left to stop it.
      let backendStillRecording = false;
      try {
        backendStillRecording = await invoke<boolean>('is_recording');
      } catch {
        // State unknown — fall through to the save attempt rather than risk
        // discarding a recording that did finish.
      }
      if (backendStillRecording) {
        // Lift the page out of the "Stopping…" state the stop click put it in;
        // the backend is still recording, so that is what the UI must show.
        recordingState.setStatus(RecordingStatus.RECORDING);
        toast.error('Could not stop the recording', {
          id: 'stop-recording-failed',
          description: `${errorText}. The recording is still running — press Stop to try again.`,
          duration: 10000,
        });
        return;
      }
      // Backend confirms stopped: salvage the meeting. If Rust already persisted it
      // the save path reconciles via the returned meeting_id; otherwise it falls
      // back to the frontend transcript state.
      onRecordingStop(true);
    } finally {
      setIsStopping(false);
    }
  }, [onRecordingStop, recordingState]);

  const handleStopRecording = useCallback(async () => {
    console.log('handleStopRecording called - isRecording:', isRecording, 'isStarting:', isStarting, 'isStopping:', isStopping);
    if (!isRecording || isStarting || isStopping) {
      console.log('Early return from handleStopRecording due to state check');
      return;
    }

    console.log('Stopping recording...');

    // Notify parent immediately (for UI state updates)
    onStopInitiated?.();

    setIsStopping(true);

    // Immediately trigger the stop action
    await stopRecordingAction();
  }, [isRecording, isStarting, isStopping, stopRecordingAction, onStopInitiated]);

  const handlePauseRecording = useCallback(async () => {
    if (!isRecording || isPaused || isPausing) return;

    console.log('Pausing recording...');
    setIsPausing(true);

    try {
      await invoke('pause_recording');
      // isPaused state now managed by RecordingStateContext via events
      console.log('Recording paused successfully');
    } catch (error) {
      console.error('Failed to pause recording:', error);
      toast.error('Failed to pause recording. Please check the console for details.');
    } finally {
      setIsPausing(false);
    }
  }, [isRecording, isPaused, isPausing]);

  const handleResumeRecording = useCallback(async () => {
    if (!isRecording || !isPaused || isResuming) return;

    console.log('Resuming recording...');
    setIsResuming(true);

    try {
      await invoke('resume_recording');
      // isPaused state now managed by RecordingStateContext via events
      console.log('Recording resumed successfully');
    } catch (error) {
      console.error('Failed to resume recording:', error);
      toast.error('Failed to resume recording. Please check the console for details.');
    } finally {
      setIsResuming(false);
    }
  }, [isRecording, isPaused, isResuming]);

  const handleToggleMicMute = useCallback(async () => {
    if (!isRecording || isTogglingMute || isStopping) return;

    const next = !isMicMuted;
    setIsTogglingMute(true);
    try {
      await invoke('set_microphone_muted', { muted: next });
      // isMicMuted is driven by the microphone-muted event via RecordingStateContext
      toast.success(next ? 'Microphone off — nothing from your mic is being recorded' : 'Microphone back on');
    } catch (error) {
      console.error('Failed to toggle microphone mute:', error);
      toast.error(`Failed to turn the microphone ${next ? 'off' : 'on'}`);
    } finally {
      setIsTogglingMute(false);
    }
  }, [isRecording, isMicMuted, isTogglingMute, isStopping]);

  useEffect(() => {
    return () => {
      // Cleanup on unmount if needed
    };
  }, []);

  useEffect(() => {
    console.log('Setting up recording event listeners');
    let unsubscribes: (() => void)[] = [];

    const setupListeners = async () => {
      try {
        // Transcript error listener - handles both regular and actionable errors
        const transcriptErrorUnsubscribe = await listen('transcript-error', (event) => {
          console.log('transcript-error event received:', event);
          console.error('Transcription error received:', event.payload);
          const errorMessage = event.payload as string;

          setTranscriptionErrors(prev => {
            const newCount = prev + 1;
            console.log('Transcription error count incremented:', newCount);
            return newCount;
          });
          setIsProcessing(false);
          // Save (true), don't discard: a transient transcript error must not cost the
          // whole meeting. If there is content it is persisted; if not, the stop flow
          // simply goes idle.
          console.log('Calling onRecordingStop(true) due to transcript error (save partial)');
          onRecordingStop(true);
          if (onTranscriptionError) {
            onTranscriptionError(errorMessage);
          }
        });

        // Transcription error listener - handles structured error objects with actionable flag
        const transcriptionErrorUnsubscribe = await listen('transcription-error', (event) => {
          console.log('transcription-error event received:', event);
          console.error('Transcription error received:', event.payload);

          let errorMessage: string;
          let isActionable = false;

          if (typeof event.payload === 'object' && event.payload !== null) {
            const payload = event.payload as { error: string, userMessage: string, actionable: boolean };
            errorMessage = payload.userMessage || payload.error;
            isActionable = payload.actionable || false;
          } else {
            errorMessage = String(event.payload);
          }

          setTranscriptionErrors(prev => {
            const newCount = prev + 1;
            console.log('Transcription error count incremented:', newCount);
            return newCount;
          });
          setIsProcessing(false);
          // Only a start-time, actionable model-not-ready error should abort without
          // saving (there is no content yet, and the page shows the model selector).
          // A runtime/transient transcription error must save the partial meeting.
          console.log(`Calling onRecordingStop(${!isActionable}) due to transcription error`);
          onRecordingStop(!isActionable);

          // For actionable errors (like model loading failures), the main page will handle showing the model selector
          // For regular errors, they are handled by useModalState global listener which shows a toast
          // We don't want to show a modal (via onTranscriptionError) AND a toast, so we skip the callback here
          /* if (onTranscriptionError && !isActionable) {
            onTranscriptionError(errorMessage);
          } */
        });

        // Pause/Resume events are now handled by RecordingStateContext
        // No need for duplicate listeners here

        // Speech detected listener - for UX feedback when VAD detects speech
        const speechDetectedUnsubscribe = await listen('speech-detected', (event) => {
          console.log('speech-detected event received:', event);
          setSpeechDetected(true);
        });

        // Continuous "speech detected" derived from REAL levels (self-clears after
        // ~600ms of silence), so the badge reflects the current state, not a one-shot.
        let speechDecay: ReturnType<typeof setTimeout> | null = null;
        const levelsUnsubscribe = await listen<{ mic_rms: number; system_rms: number }>(
          'recording-levels',
          (event) => {
            const rms = Math.max(event.payload.mic_rms ?? 0, (event.payload.system_rms ?? 0) * 0.7);
            if (rms > 0.008) {
              setSpeechDetected(true);
              if (speechDecay) clearTimeout(speechDecay);
              speechDecay = setTimeout(() => setSpeechDetected(false), 600);
            }
          }
        );

        // Device disconnect/reconnect banners (from the Rust recording supervisor).
        const deviceDisconnectedUnsubscribe = await listen<{ device_name?: string; device_type?: string }>(
          'device-disconnected',
          (event) => {
            const name = event.payload?.device_name || 'A device';
            const isSystem = (event.payload?.device_type || '').toLowerCase().includes('system');
            setDeviceError({
              title: isSystem ? 'System audio disconnected' : 'Microphone disconnected',
              message: `${name} was disconnected. Recording continues, but this source may be silent until it reconnects.`,
            });
          }
        );
        const deviceReconnectedUnsubscribe = await listen('device-reconnected', () => {
          setDeviceError(null);
          toast.success('Audio device reconnected', { id: 'device-reconnected', duration: 3000 });
        });

        // System audio couldn't be captured — recording is mic-only.
        const systemAudioUnavailableUnsubscribe = await listen<{ device_name?: string }>(
          'system-audio-unavailable',
          (event) => {
            setDeviceError({
              title: 'System audio unavailable',
              message: `Could not capture system audio${event.payload?.device_name ? ` from ${event.payload.device_name}` : ''}. Recording microphone only.`,
            });
          }
        );

        // Silence watchdog — likely a dead mic even if the device is still enumerated.
        const silenceWarningUnsubscribe = await listen<{ minutes?: number }>(
          'recording-silence-warning',
          (event) => {
            const mins = event.payload?.minutes ?? 5;
            setDeviceError({
              title: 'No speech detected',
              message: `No speech has been detected for ${mins} minutes — is your microphone working?`,
            });
          }
        );

        // Clear the HUD state when the recording stops.
        const recordingStoppedUnsubscribe = await listen('recording-stopped', () => {
          if (speechDecay) clearTimeout(speechDecay);
          setDeviceError(null);
          setSpeechDetected(false);
        });

        // Recording error listener - surfaces backend audio/persistence failures
        // (dead device, disk full, encode/finalize failure) that used to be log-only.
        // Payload is either a plain string (RecordingState error callback) or an
        // object { kind, message } (finalize/save failures). This is informational:
        // it does NOT stop the recording, so an in-progress meeting is never discarded.
        const recordingErrorUnsubscribe = await listen('recording-error', (event) => {
          console.error('recording-error event received:', event.payload);
          const payload = event.payload as unknown;
          const isObj = typeof payload === 'object' && payload !== null;
          const message =
            typeof payload === 'string'
              ? payload
              : (payload as { message?: string })?.message || 'A recording error occurred';
          const kind = isObj ? (payload as { kind?: string }).kind : undefined;
          const meetingFolder = isObj
            ? (payload as { meeting_folder?: string | null }).meeting_folder
            : undefined;

          // Audio finalize failed, but the meeting + checkpoint audio are on disk.
          // Offer a one-click merge instead of letting the user discover silent audio loss.
          if (kind === 'audio_save_failed' && meetingFolder) {
            toast.error('Recording saved, but audio finalization failed', {
              id: 'recording-error',
              description: message,
              duration: Infinity,
              action: {
                label: 'Merge audio',
                onClick: () => {
                  toast.loading('Merging audio…', { id: 'merge-audio' });
                  invoke('recover_audio_from_checkpoints', {
                    meetingFolder,
                    sampleRate: 48000,
                  })
                    .then(() => toast.success('Audio recovered', { id: 'merge-audio' }))
                    .catch((e) =>
                      toast.error('Audio merge failed', {
                        id: 'merge-audio',
                        description: String(e),
                      })
                    );
                },
              },
            });
            return;
          }

          // Throttle: a dead device makes the backend report recoverable errors
          // continuously; without this the repeated toast floods sonner and React bails
          // with "Maximum update depth exceeded". Show at most once every 3s.
          const now = Date.now();
          if (now - lastRecordingErrorToastRef.current < 3000) return;
          lastRecordingErrorToastRef.current = now;
          // Stable id so repeated errors replace rather than stack into a toast pile.
          toast.error('Recording problem', { id: 'recording-error', description: message });
        });

        unsubscribes = [
          transcriptErrorUnsubscribe,
          transcriptionErrorUnsubscribe,
          speechDetectedUnsubscribe,
          levelsUnsubscribe,
          deviceDisconnectedUnsubscribe,
          deviceReconnectedUnsubscribe,
          systemAudioUnavailableUnsubscribe,
          silenceWarningUnsubscribe,
          recordingStoppedUnsubscribe,
          recordingErrorUnsubscribe,
          () => { if (speechDecay) clearTimeout(speechDecay); },
        ];
        console.log('Recording event listeners set up successfully');
      } catch (error) {
        console.error('Failed to set up recording event listeners:', error);
      }
    };

    setupListeners();

    return () => {
      console.log('Cleaning up recording event listeners');
      unsubscribes.forEach(unsubscribe => {
        if (unsubscribe && typeof unsubscribe === 'function') {
          unsubscribe();
        }
      });
    };
  }, [onRecordingStop, onTranscriptionError]);

  return (
    <TooltipProvider>
      <div className="flex flex-col space-y-2">
        <div className="flex items-center space-x-2 bg-card/85 backdrop-blur-md border border-border rounded-full shadow-glass px-4 py-2">
          {isProcessing && !isParentProcessing ? (
            <div className="flex items-center space-x-2">
              <div className="animate-spin rounded-full h-5 w-5 border-b-2 border-brand"></div>
              <span className="text-sm text-muted-foreground">Processing recording...</span>
            </div>
          ) : (
            <>
              {showPlayback ? (
                <>
                  <button
                    onClick={handleStartRecording}
                    className="w-10 h-10 flex items-center justify-center bg-primary rounded-full text-primary-foreground hover:bg-brand-hover transition-colors"
                  >
                    <Mic size={16} />
                  </button>

                  <div className="w-px h-6 bg-border mx-1" />

                  <div className="flex items-center space-x-1 mx-2">
                    <div className="text-sm text-muted-foreground font-mono min-w-[40px]">
                      {formatTime(currentTime)}
                    </div>
                    <div
                      className="relative w-24 h-1 bg-muted rounded-full"
                    >
                      <div
                        className="absolute h-full bg-brand rounded-full"
                        style={{ width: `${progress}%` }}
                      />
                    </div>
                    <div className="text-sm text-muted-foreground font-mono min-w-[40px]">
                      {formatTime(duration)}
                    </div>
                  </div>

                  <button
                    className="w-10 h-10 flex items-center justify-center bg-muted rounded-full text-muted-foreground cursor-not-allowed"
                    disabled
                  >
                    <Play size={16} />
                  </button>
                </>
              ) : (
                <>
                  {!isRecording ? (
                    // Start recording button
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          onClick={() => {
                            handleStartRecording();
                          }}
                          disabled={isStarting || isProcessing || isRecordingDisabled || isValidatingModel}
                          className={`w-12 h-12 flex items-center justify-center ${isStarting || isProcessing || isValidatingModel
                            ? 'bg-muted text-muted-foreground'
                            : 'bg-primary text-primary-foreground hover:bg-brand-hover shadow-glow'
                            } rounded-full transition-colors relative`}
                        >
                          {isStarting || isValidatingModel ? (
                            <div className="animate-spin rounded-full h-5 w-5 border-b-2 border-current"></div>
                          ) : (
                            <Mic size={20} />
                          )}
                        </button>
                      </TooltipTrigger>
                      <TooltipContent>
                        <p>Start recording</p>
                      </TooltipContent>
                    </Tooltip>
                  ) : (
                    // Recording controls (mic kill switch + pause/resume + stop)
                    <>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={handleToggleMicMute}
                            disabled={isTogglingMute || isStopping}
                            aria-pressed={isMicMuted}
                            aria-label={isMicMuted ? 'Turn microphone back on' : 'Turn microphone off'}
                            className={`w-10 h-10 flex items-center justify-center ${
                              isTogglingMute || isStopping
                                ? 'bg-muted border-2 border-border text-muted-foreground/50'
                                : isMicMuted
                                  ? 'bg-destructive/15 border-2 border-destructive text-destructive hover:bg-destructive/25'
                                  : 'bg-transparent border-2 border-border text-muted-foreground hover:border-muted-foreground hover:bg-accent'
                            } rounded-full transition-colors relative`}
                          >
                            {isMicMuted ? <MicOff size={16} /> : <Mic size={16} />}
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>
                          <p>
                            {isMicMuted
                              ? 'Microphone off — turn it back on'
                              : 'Turn the microphone off (system audio keeps recording)'}
                          </p>
                        </TooltipContent>
                      </Tooltip>

                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={() => {
                              if (isPaused) {
                                handleResumeRecording();
                              } else {
                                handlePauseRecording();
                              }
                            }}
                            disabled={isPausing || isResuming || isStopping}
                            className={`w-10 h-10 flex items-center justify-center ${isPausing || isResuming || isStopping
                              ? 'bg-muted border-2 border-border text-muted-foreground/50'
                              : 'bg-transparent border-2 border-border text-muted-foreground hover:border-muted-foreground hover:bg-accent'
                              } rounded-full transition-colors relative`}
                          >
                            {isPaused ? <Play size={16} /> : <Pause size={16} />}
                            {(isPausing || isResuming) && (
                              <div className="absolute -top-8 text-muted-foreground font-medium text-xs">
                                {isPausing ? 'Pausing...' : 'Resuming...'}
                              </div>
                            )}
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>
                          <p>{isPaused ? 'Resume recording' : 'Pause recording'}</p>
                        </TooltipContent>
                      </Tooltip>

                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={() => {
                              handleStopRecording();
                            }}
                            disabled={isStopping || isPausing || isResuming}
                            className={`w-10 h-10 flex items-center justify-center ${isStopping || isPausing || isResuming
                              ? 'bg-muted text-muted-foreground'
                              : 'bg-destructive text-destructive-foreground hover:bg-destructive/90'
                              } rounded-full transition-colors relative`}
                          >
                            {/* Live pulse ring while actively recording */}
                            {!isPaused && !isStopping && (
                              <motion.span
                                className="absolute inset-0 rounded-full border-2 border-destructive"
                                animate={{ scale: [1, 1.45], opacity: [0.6, 0] }}
                                transition={{ repeat: Infinity, duration: 1.6, ease: 'easeOut' }}
                              />
                            )}
                            <Square size={16} />
                            {isStopping && (
                              <div className="absolute -top-8 text-muted-foreground font-medium text-xs">
                                Stopping...
                              </div>
                            )}
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>
                          <p>Stop recording</p>
                        </TooltipContent>
                      </Tooltip>
                    </>
                  )}

                  <div className="flex items-center space-x-1 mx-4">
                    {barHeights.map((height, index) => (
                      <div
                        key={index}
                        className={`w-1 rounded-full transition-all duration-200 ${isPaused ? 'bg-warning' : isRecording ? 'bg-destructive' : 'bg-brand/50'
                          }`}
                        style={{
                          height: isRecording && !isPaused ? height : '4px',
                          opacity: isPaused ? 0.6 : 1,
                        }}
                      />
                    ))}
                  </div>
                </>
              )}
            </>
          )}
        </div>

        {/* Show validation status only */}
        {isValidatingModel && (
          <div className="text-xs text-muted-foreground text-center mt-2">
            Validating speech recognition...
          </div>
        )}

        {/* Live "speech detected" badge (self-clears after ~600ms of silence) */}
        {speechDetected && !isMicMuted && (
          <div className="flex items-center justify-center gap-1.5 text-xs text-success mt-2">
            <span className="h-2 w-2 rounded-full bg-success animate-pulse" />
            Speech detected
          </div>
        )}

        {/* Persistent reminder while the mic kill switch is engaged. Deliberately
            unmissable: forgetting it is on would silently lose your own audio for
            the rest of the meeting. */}
        {isRecording && isMicMuted && (
          <div className="flex items-center justify-center gap-1.5 text-xs text-destructive font-medium mt-2">
            <MicOff size={14} />
            Microphone off — only system audio is being recorded
          </div>
        )}

        {/* Device error alert */}
        {deviceError && (
          <Alert variant="destructive" className="mt-4 border-destructive/40 bg-destructive/10">
            <AlertCircle className="h-5 w-5 text-destructive" />
            <button
              onClick={() => setDeviceError(null)}
              className="absolute right-3 top-3 text-destructive hover:text-destructive/80 transition-colors"
              aria-label="Close alert"
            >
              <X className="h-4 w-4" />
            </button>
            <AlertTitle className="text-destructive font-semibold mb-2">
              {deviceError.title}
            </AlertTitle>
            <AlertDescription className="text-destructive/90">
              {deviceError.message.split('\n').map((line, i) => (
                <div key={i} className={i > 0 ? 'ml-2' : ''}>
                  {line}
                </div>
              ))}
            </AlertDescription>
          </Alert>
        )}

        {/* {showPlayback && recordingPath && (
        <div className="text-sm text-gray-600 px-4">
          Recording saved to: {recordingPath}
        </div>
      )} */}
      </div>
    </TooltipProvider>
  );
};