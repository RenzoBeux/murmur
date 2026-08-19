'use client';

import { useState, useEffect } from 'react';
import { motion } from 'framer-motion';
import { RecordingControls } from '@/components/RecordingControls';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { usePermissionCheck } from '@/hooks/usePermissionCheck';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useConfig } from '@/contexts/ConfigContext';
import { StatusOverlays } from '@/app/_components/StatusOverlays';
import { SettingsModals } from './_components/SettingsModal';
import { TranscriptPanel } from './_components/TranscriptPanel';
import { LiveChatPanel } from './_components/LiveChatPanel';
import { useModalState } from '@/hooks/useModalState';
import { useRecordingStateSync } from '@/hooks/useRecordingStateSync';
import { LanguageQuickPick } from '@/components/LanguageQuickPick';
import { useRecordingStart } from '@/hooks/useRecordingStart';
import { useRecordingStop } from '@/hooks/useRecordingStop';
import { useTranscriptRecovery } from '@/hooks/useTranscriptRecovery';
import { useFilesystemRecovery } from '@/hooks/useFilesystemRecovery';
import { TranscriptRecovery } from '@/components/TranscriptRecovery';
import { indexedDBService } from '@/services/indexedDBService';
import { toast } from 'sonner';
import { listen } from '@tauri-apps/api/event';
import { useRouter } from 'next/navigation';

const WAVEFORM_BAR_COUNT = 7;

export default function Home() {
  // Local page state (not moved to contexts)
  const [isRecording, setIsRecordingState] = useState(false);
  const [barHeights, setBarHeights] = useState<string[]>(() => Array(WAVEFORM_BAR_COUNT).fill('4px'));
  const [showRecoveryDialog, setShowRecoveryDialog] = useState(false);
  // Ask-AI side panel. sessionStorage keeps it open across a webview reload
  // mid-recording, matching the transcript's own rehydration.
  const [showAskAi, setShowAskAi] = useState(
    () => typeof window !== 'undefined' && sessionStorage.getItem('ask_ai_panel_open') === '1'
  );
  const toggleAskAi = () => {
    setShowAskAi((prev) => {
      const next = !prev;
      sessionStorage.setItem('ask_ai_panel_open', next ? '1' : '0');
      return next;
    });
  };

  // Use contexts for state management
  const { meetingTitle } = useTranscripts();
  const { transcriptModelConfig, selectedDevices } = useConfig();
  const recordingState = useRecordingState();

  // Extract status from global state
  const { status, isStopping, isProcessing, isSaving } = recordingState;

  // Hooks
  const { hasMicrophone } = usePermissionCheck();
  const { isCollapsed: sidebarCollapsed, refetchMeetings } = useSidebar();
  const { modals, messages, showModal, hideModal } = useModalState(transcriptModelConfig);
  const { isRecordingDisabled, setIsRecordingDisabled } = useRecordingStateSync();
  const { handleRecordingStart } = useRecordingStart(isRecording, setIsRecordingState, showModal);

  // Get handleRecordingStop function and setIsStopping (state comes from global context)
  const { handleRecordingStop, setIsStopping } = useRecordingStop(
    setIsRecordingState,
    setIsRecordingDisabled
  );

  // Recovery hook
  const {
    recoverableMeetings,
    isLoading: isLoadingRecovery,
    isRecovering,
    checkForRecoverableTranscripts,
    recoverMeeting,
    loadMeetingTranscripts,
    deleteRecoverableMeeting
  } = useTranscriptRecovery();

  // Filesystem-based recovery (independent of IndexedDB): reads the transcripts.json
  // Rust writes to disk, so a meeting that never journaled to the webview is still
  // recoverable.
  const { checkForInterruptedRecordings, recoverFromFolder } = useFilesystemRecovery();

  const router = useRouter();

  // Startup recovery check
  useEffect(() => {
    const performStartupChecks = async () => {
      try {
        // Skip recovery check if currently recording or processing stop
        // This prevents the recovery dialog from showing when:
        if (recordingState.isRecording ||
          status === RecordingStatus.STOPPING ||
          status === RecordingStatus.PROCESSING_TRANSCRIPTS ||
          status === RecordingStatus.SAVING) {
          console.log('Skipping recovery check - recording in progress or processing');
          return;
        }

        // 1. Detect recoverable (unsaved) meetings FIRST, before any cleanup runs,
        //    so crash data is never at risk of being purged before it is offered.
        //    Don't skip based on sessionStorage - we need to check every time.
        await checkForRecoverableTranscripts();

        // 2. Clean up old meetings (7+ days) — only SAVED ones (deleteOldMeetings
        //    now guards on savedToSQLite so unsaved crash journals are retained).
        try {
          await indexedDBService.deleteOldMeetings(7);
        } catch (error) {
          console.warn('⚠️ Failed to clean up old meetings:', error);
        }

        // 3. Clean up saved meetings (24+ hours after save)
        try {
          await indexedDBService.deleteSavedMeetings(24);
        } catch (error) {
          console.warn('⚠️ Failed to clean up saved meetings:', error);
        }

        // 4. Filesystem recovery (independent of IndexedDB): find interrupted
        //    recordings on disk the IndexedDB dialog won't already show (dedup by
        //    folder path) and offer a one-click recover. Runs even when IndexedDB has
        //    nothing — it's the durable safety net.
        try {
          const disk = await checkForInterruptedRecordings();
          if (disk.length > 0) {
            const idbMeetings = await indexedDBService.getAllMeetings();
            const idbFolders = new Set(idbMeetings.map((m) => m.folderPath).filter(Boolean));
            const diskOnly = disk.filter((d) => !idbFolders.has(d.folder_path));
            if (diskOnly.length > 0) {
              toast.info(
                `Found ${diskOnly.length} interrupted recording${diskOnly.length > 1 ? 's' : ''} on disk`,
                {
                  id: 'filesystem-recovery',
                  description: "A recording didn't finish saving. Recover it now?",
                  duration: Infinity,
                  action: {
                    label: 'Recover',
                    onClick: async () => {
                      let recovered = 0;
                      let lastId: string | undefined;
                      for (const item of diskOnly) {
                        try {
                          const res = await recoverFromFolder(item.folder_path);
                          lastId = res.meeting_id;
                          recovered++;
                        } catch (e) {
                          console.warn('Filesystem recovery import failed:', e);
                        }
                      }
                      if (recovered > 0) {
                        await refetchMeetings();
                        toast.success(
                          `Recovered ${recovered} meeting${recovered > 1 ? 's' : ''} from disk`,
                          {
                            action: lastId
                              ? {
                                  label: 'View',
                                  onClick: () => router.push(`/meeting-details?id=${lastId}`),
                                }
                              : undefined,
                          }
                        );
                      } else {
                        toast.error('Could not recover the interrupted recording(s)');
                      }
                    },
                  },
                }
              );
            }
          }
        } catch (error) {
          console.warn('Filesystem recovery scan failed:', error);
        }
      } catch (error) {
        console.error('Failed to perform startup checks:', error);
      }
    };

    performStartupChecks();
  }, [checkForRecoverableTranscripts, checkForInterruptedRecordings, recoverFromFolder, recordingState.isRecording, status]);

  // Watch for recoverable meetings changes and show dialog once per session
  useEffect(() => {
    // Only show dialog if we have meetings and haven't shown it yet this session
    if (recoverableMeetings.length > 0) {
      const shownThisSession = sessionStorage.getItem('recovery_dialog_shown');
      if (!shownThisSession) {
        setShowRecoveryDialog(true);
        sessionStorage.setItem('recovery_dialog_shown', 'true');
      }
    }
  }, [recoverableMeetings]);

  // Handle recovery with toast notifications and navigation
  const handleRecovery = async (meetingId: string) => {
    try {
      const result = await recoverMeeting(meetingId);

      if (result.success) {
        toast.success('Meeting recovered successfully!', {
          description: result.audioRecoveryStatus?.status === 'success'
            ? 'Transcripts and audio recovered'
            : 'Transcripts recovered (no audio available)',
          action: result.meetingId ? {
            label: 'View Meeting',
            onClick: () => {
              router.push(`/meeting-details?id=${result.meetingId}`);
            }
          } : undefined,
          duration: 10000,
        });

        // Refresh sidebar to show the newly recovered meeting
        await refetchMeetings();

        // If no more recoverable meetings, clear session flag so dialog can show again
        if (recoverableMeetings.length === 0) {
          sessionStorage.removeItem('recovery_dialog_shown');
        }

        // Auto-navigate after a short delay
        if (result.meetingId) {
          setTimeout(() => {
            router.push(`/meeting-details?id=${result.meetingId}`);
          }, 2000);
        }
      }
    } catch (error) {
      toast.error('Failed to recover meeting', {
        description: error instanceof Error ? error.message : 'Unknown error occurred',
      });
      throw error;
    }
  };

  // Handle dialog close - clear session flag if no meetings left
  const handleDialogClose = () => {
    setShowRecoveryDialog(false);
    // If user closes dialog and there are no more meetings, clear the flag
    // This allows the dialog to show again next session if new meetings appear
    if (recoverableMeetings.length === 0) {
      sessionStorage.removeItem('recovery_dialog_shown');
    }
  };

  // Drive the recording waveform from REAL mic/system loudness (recording-levels,
  // ~10 Hz from the Rust supervisor) instead of fake Math.random() bars, so the meter
  // tracks actual audio and flatlines on silence / a dead mic.
  useEffect(() => {
    if (!recordingState.isRecording) return;
    let mounted = true;
    let unlisten: (() => void) | undefined;

    listen<{ mic_rms: number; system_rms: number }>('recording-levels', (event) => {
      const rms = Math.max(event.payload.mic_rms ?? 0, (event.payload.system_rms ?? 0) * 0.7);
      // Perceptual scaling (sqrt lifts quiet speech); clamp to a legible px range.
      const scaled = Math.min(1, Math.sqrt(rms) * 3);
      const px = 4 + Math.round(scaled * 24); // 4..28px
      setBarHeights((prev) => {
        const next = prev.slice(1);
        next.push(px + 'px');
        return next;
      });
    }).then((fn) => {
      if (mounted) unlisten = fn;
      else fn();
    });

    return () => {
      mounted = false;
      if (unlisten) unlisten();
      setBarHeights(Array(WAVEFORM_BAR_COUNT).fill('4px')); // flatline on stop
    };
  }, [recordingState.isRecording]);

  // Computed values using global status
  const isProcessingStop = status === RecordingStatus.PROCESSING_TRANSCRIPTS || isProcessing;

  return (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3, ease: 'easeOut' }}
      className="flex flex-col h-[calc(100vh-var(--titlebar-height))] bg-background"
    >
      {/* All Modals supported*/}
      <SettingsModals
        modals={modals}
        messages={messages}
        onClose={hideModal}
      />

      {/* Recovery Dialog */}
      <TranscriptRecovery
        isOpen={showRecoveryDialog}
        onClose={handleDialogClose}
        recoverableMeetings={recoverableMeetings}
        onRecover={handleRecovery}
        onDelete={deleteRecoverableMeeting}
        onLoadPreview={loadMeetingTranscripts}
      />
      <div className="flex flex-1 overflow-hidden">
        <TranscriptPanel
          isProcessingStop={isProcessingStop}
          isStopping={isStopping}
          showModal={showModal}
          isAskAiOpen={showAskAi}
          onToggleAskAi={toggleAskAi}
        />

        {/* Ask AI side panel — mirrors the meeting-details two-column layout */}
        {showAskAi && (
          <div className="w-[380px] xl:w-[420px] shrink-0 border-l border-border flex flex-col">
            <LiveChatPanel onClose={toggleAskAi} />
          </div>
        )}

        {/* Recording controls - only show when permissions are granted or already recording and not showing status messages */}
        {(hasMicrophone || isRecording) &&
          status !== RecordingStatus.PROCESSING_TRANSCRIPTS &&
          status !== RecordingStatus.SAVING && (
            // Inset by the Ask-AI drawer width so the pill stays centered over the
            // transcript instead of floating over (and under) the drawer.
            <div className={`fixed bottom-12 left-0 z-10 ${showAskAi ? 'right-[380px] xl:right-[420px]' : 'right-0'}`}>
              <div
                className={`flex justify-center pl-3 md:pl-8 transition-[margin] duration-300 ${
                  sidebarCollapsed ? 'ml-16' : 'ml-16 md:ml-64'
                }`}
              >
                <div className="w-full px-4 md:px-0 md:w-2/3 max-w-[750px] flex justify-center">
                  <div className="flex items-center">
                    <RecordingControls
                      isRecording={recordingState.isRecording}
                      onRecordingStop={(callApi = true) => handleRecordingStop(callApi)}
                      onRecordingStart={handleRecordingStart}
                      onTranscriptReceived={() => { }} // Not actually used by RecordingControls
                      onStopInitiated={() => setIsStopping(true)}
                      barHeights={barHeights}
                      onTranscriptionError={(message) => {
                        showModal('errorAlert', message);
                      }}
                      isRecordingDisabled={isRecordingDisabled}
                      isParentProcessing={isProcessingStop}
                      selectedDevices={selectedDevices}
                      meetingName={meetingTitle}
                    />
                    {!recordingState.isRecording && <LanguageQuickPick className="ml-3" />}
                  </div>
                </div>
              </div>
            </div>
          )}

        {/* Status Overlays - Processing and Saving */}
        <StatusOverlays
          isProcessing={status === RecordingStatus.PROCESSING_TRANSCRIPTS && !recordingState.isRecording}
          isSaving={status === RecordingStatus.SAVING}
          sidebarCollapsed={sidebarCollapsed}
          askAiOpen={showAskAi}
        />
      </div>
    </motion.div>
  );
}
