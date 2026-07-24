"use client";
import { useState, useEffect, useMemo, useRef } from 'react';
import { motion } from 'framer-motion';
import { Summary, SummaryResponse, Transcript } from '@/types';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { TranscriptPanel } from '@/components/MeetingDetails/TranscriptPanel';
import { SummaryPanel } from '@/components/MeetingDetails/SummaryPanel';
import { ChatPanel } from '@/components/MeetingDetails/ChatPanel';
import { AttachmentsPanel } from '@/components/MeetingDetails/AttachmentsPanel';
import { ModelConfig } from '@/components/ModelSettingsModal';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { FileText, MessageSquare, Paperclip } from 'lucide-react';
import { FileDropClaim, useFileDropTarget } from '@/contexts/FileDropContext';

// Custom hooks
import { useAttachments } from '@/hooks/meeting-details/useAttachments';
import { useMeetingData } from '@/hooks/meeting-details/useMeetingData';
import { useSummaryGeneration } from '@/hooks/meeting-details/useSummaryGeneration';
import { useTemplates } from '@/hooks/meeting-details/useTemplates';
import { useCopyOperations } from '@/hooks/meeting-details/useCopyOperations';
import { useExportOperations } from '@/hooks/meeting-details/useExportOperations';
import { useMeetingOperations } from '@/hooks/meeting-details/useMeetingOperations';
import { useConfig } from '@/contexts/ConfigContext';

export default function PageContent({
  meeting,
  summaryData,
  shouldAutoGenerate = false,
  onAutoGenerateComplete,
  onMeetingUpdated,
  onRefetchTranscripts,
  // Pagination props for efficient transcript loading
  segments,
  hasMore,
  isLoadingMore,
  totalCount,
  loadedCount,
  onLoadMore,
  applyLocalMutation,
}: {
  meeting: any;
  summaryData: Summary | null;
  shouldAutoGenerate?: boolean;
  onAutoGenerateComplete?: () => void;
  onMeetingUpdated?: () => Promise<void>;
  onRefetchTranscripts?: () => Promise<void>;
  // Pagination props
  segments?: any[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  loadedCount?: number;
  onLoadMore?: () => void;
  applyLocalMutation?: (mutator: (prev: Transcript[]) => Transcript[]) => void;
}) {
  console.log('📄 PAGE CONTENT: Initializing with data:', {
    meetingId: meeting.id,
    summaryDataKeys: summaryData ? Object.keys(summaryData) : null,
    transcriptsCount: meeting.transcripts?.length
  });

  // State
  const [customPrompt, setCustomPrompt] = useState<string>('');
  const [attendees, setAttendees] = useState<string>('');
  const [isRecording] = useState(false);
  const [summaryResponse] = useState<SummaryResponse | null>(null);

  // Ref to store the modal open function from SummaryGeneratorButtonGroup
  const openModelSettingsRef = useRef<(() => void) | null>(null);

  // Get model config from ConfigContext
  const { modelConfig, setModelConfig, isModelConfigLoading } = useConfig();

  // Custom hooks
  const meetingData = useMeetingData({ meeting, summaryData, onMeetingUpdated });
  const templates = useTemplates();

  // Attachments (photos/files for context). Registered as the page's file-drop
  // target here — not in AttachmentsPanel — because Radix unmounts inactive
  // tabs, and drops should attach whichever tab is active.
  const attachmentsApi = useAttachments(meeting.id);
  const { addFromPaths } = attachmentsApi;
  const dropClaim = useMemo<FileDropClaim>(
    () => ({
      onDrop: (paths: string[]) => {
        addFromPaths(paths);
        return true;
      },
      overlay: {
        title: 'Drop files to attach',
        subtitle: 'Photos and documents are added to this meeting',
      },
    }),
    [addFromPaths],
  );
  useFileDropTarget(dropClaim);

  // Load the persisted attendee roster for this meeting. The backend injects it
  // into summary prompts so the LLM uses canonical name spellings.
  useEffect(() => {
    let cancelled = false;
    invoke<string | null>('api_get_meeting_attendees', { meetingId: meeting.id })
      .then((value) => {
        if (!cancelled) setAttendees(value ?? '');
      })
      .catch((error) => console.error('Failed to load meeting attendees:', error));
    return () => {
      cancelled = true;
    };
  }, [meeting.id]);

  const handleAttendeesSave = async (value: string) => {
    try {
      await invoke('api_save_meeting_attendees', {
        meetingId: meeting.id,
        attendees: value.trim() || null,
      });
    } catch (error) {
      console.error('Failed to save meeting attendees:', error);
      toast.error('Failed to save attendees');
    }
  };

  // Callback to register the modal open function
  const handleRegisterModalOpen = (openFn: () => void) => {
    console.log('📝 Registering modal open function in PageContent');
    openModelSettingsRef.current = openFn;
  };

  // Callback to trigger modal open (called from error handler)
  const handleOpenModelSettings = () => {
    console.log('🔔 Opening model settings from PageContent');
    if (openModelSettingsRef.current) {
      openModelSettingsRef.current();
    } else {
      console.warn('⚠️ Modal open function not yet registered');
    }
  };

  // Save model config to backend database and sync via event
  const handleSaveModelConfig = async (config?: ModelConfig) => {
    if (!config) return;
    try {
      await invoke('api_save_model_config', {
        provider: config.provider,
        model: config.model,
        whisperModel: config.whisperModel,
        apiKey: config.apiKey ?? null,
        ollamaEndpoint: config.ollamaEndpoint ?? null,
        lmStudioEndpoint: config.lmStudioEndpoint ?? null,
      });

      // Emit event so ConfigContext and other listeners stay in sync
      const { emit } = await import('@tauri-apps/api/event');
      await emit('model-config-updated', config);

      toast.success('Model settings saved successfully');
    } catch (error) {
      console.error('Failed to save model config:', error);
      toast.error('Failed to save model settings');
    }
  };

  const summaryGeneration = useSummaryGeneration({
    meeting,
    transcripts: meetingData.transcripts,
    modelConfig: modelConfig,
    isModelConfigLoading,
    selectedTemplate: templates.selectedTemplate,
    onMeetingUpdated,
    updateMeetingTitle: meetingData.updateMeetingTitle,
    setAiSummary: meetingData.setAiSummary,
    onOpenModelSettings: handleOpenModelSettings,
  });

  const copyOperations = useCopyOperations({
    meeting,
    transcripts: meetingData.transcripts,
    meetingTitle: meetingData.meetingTitle,
    aiSummary: meetingData.aiSummary,
    blockNoteSummaryRef: meetingData.blockNoteSummaryRef,
  });

  const exportOperations = useExportOperations({
    meeting,
    meetingTitle: meetingData.meetingTitle,
    aiSummary: meetingData.aiSummary,
    blockNoteSummaryRef: meetingData.blockNoteSummaryRef,
  });

  const meetingOperations = useMeetingOperations({
    meeting,
  });

  // Auto-generate summary when flag is set
  useEffect(() => {
    let cancelled = false;

    const autoGenerate = async () => {
      if (shouldAutoGenerate && meetingData.transcripts.length > 0 && !cancelled) {
        console.log(`🤖 Auto-generating summary with ${modelConfig.provider}/${modelConfig.model}...`);
        await summaryGeneration.handleGenerateSummary('');

        // Notify parent that auto-generation is complete (only if not cancelled)
        if (onAutoGenerateComplete && !cancelled) {
          onAutoGenerateComplete();
        }
      }
    };

    autoGenerate();

    // Cleanup: cancel if component unmounts or meeting changes
    return () => {
      cancelled = true;
    };
    // isModelConfigLoading is a dep so that if auto-generate is requested before
    // the model config finishes loading (handleGenerateSummary bails while
    // loading), it re-runs and generates once the real config is in.
  }, [shouldAutoGenerate, meeting.id, isModelConfigLoading]); // Re-run if meeting/config changes

  return (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3, ease: 'easeOut' }}
      className="flex flex-col h-[calc(100vh-var(--titlebar-height))] bg-background"
    >
      <div className="flex flex-1 overflow-hidden">
        <TranscriptPanel
          transcripts={meetingData.transcripts}
          customPrompt={customPrompt}
          onPromptChange={setCustomPrompt}
          attendees={attendees}
          onAttendeesChange={setAttendees}
          onAttendeesSave={handleAttendeesSave}
          onCopyTranscript={copyOperations.handleCopyTranscript}
          onOpenMeetingFolder={meetingOperations.handleOpenMeetingFolder}
          onExportMarkdown={exportOperations.handleExportMarkdown}
          hasSummary={!!meetingData.aiSummary}
          isRecording={isRecording}
          disableAutoScroll={true}
          // Pagination props for efficient loading
          usePagination={true}
          segments={segments}
          hasMore={hasMore}
          isLoadingMore={isLoadingMore}
          totalCount={totalCount}
          loadedCount={loadedCount}
          onLoadMore={onLoadMore}
          // Retranscription props
          meetingId={meeting.id}
          meetingFolderPath={meeting.folder_path}
          onRefetchTranscripts={onRefetchTranscripts}
          applyLocalMutation={applyLocalMutation}
        />
        <Tabs defaultValue="summary" className="flex-1 min-w-0 flex flex-col bg-background overflow-hidden">
          <div className="flex items-center justify-center border-b border-border px-4 py-2">
            <TabsList>
              <TabsTrigger value="summary" className="gap-1.5">
                <FileText className="h-4 w-4" /> Summary
              </TabsTrigger>
              <TabsTrigger value="chat" className="gap-1.5">
                <MessageSquare className="h-4 w-4" /> Chat
              </TabsTrigger>
              <TabsTrigger value="attachments" className="gap-1.5">
                <Paperclip className="h-4 w-4" /> Attachments
                {attachmentsApi.attachments.length > 0 && (
                  <span className="rounded-full bg-muted px-1.5 text-xs text-muted-foreground">
                    {attachmentsApi.attachments.length}
                  </span>
                )}
              </TabsTrigger>
            </TabsList>
          </div>
          <TabsContent value="summary" className="flex-1 min-h-0 mt-0 flex flex-col overflow-hidden">
            <SummaryPanel
              meeting={meeting}
              meetingTitle={meetingData.meetingTitle}
              onTitleChange={meetingData.handleTitleChange}
              isEditingTitle={meetingData.isEditingTitle}
              onStartEditTitle={() => meetingData.setIsEditingTitle(true)}
              onFinishEditTitle={() => meetingData.setIsEditingTitle(false)}
              isTitleDirty={meetingData.isTitleDirty}
              summaryRef={meetingData.blockNoteSummaryRef}
              isSaving={meetingData.isSaving}
              onSaveAll={meetingData.saveAllChanges}
              onCopySummary={copyOperations.handleCopySummary}
              onOpenFolder={meetingOperations.handleOpenMeetingFolder}
              aiSummary={meetingData.aiSummary}
              summaryStatus={summaryGeneration.summaryStatus}
              transcripts={meetingData.transcripts}
              modelConfig={modelConfig}
              setModelConfig={setModelConfig}
              onSaveModelConfig={handleSaveModelConfig}
              onGenerateSummary={summaryGeneration.handleGenerateSummary}
              onStopGeneration={summaryGeneration.handleStopGeneration}
              customPrompt={customPrompt}
              summaryResponse={summaryResponse}
              onSaveSummary={meetingData.handleSaveSummary}
              onSummaryChange={meetingData.handleSummaryChange}
              onDirtyChange={meetingData.setIsSummaryDirty}
              summaryError={summaryGeneration.summaryError}
              onRegenerateSummary={summaryGeneration.handleRegenerateSummary}
              getSummaryStatusMessage={summaryGeneration.getSummaryStatusMessage}
              availableTemplates={templates.availableTemplates}
              selectedTemplate={templates.selectedTemplate}
              onTemplateSelect={templates.handleTemplateSelection}
              isModelConfigLoading={isModelConfigLoading}
              onOpenModelSettings={handleRegisterModalOpen}
            />
          </TabsContent>
          <TabsContent value="chat" className="flex-1 min-h-0 mt-0 overflow-hidden">
            <ChatPanel
              meetingId={meeting.id}
              hasTranscripts={meetingData.transcripts.length > 0}
            />
          </TabsContent>
          <TabsContent value="attachments" className="flex-1 min-h-0 mt-0 overflow-hidden">
            <AttachmentsPanel
              attachmentsApi={attachmentsApi}
              hasSummary={!!meetingData.aiSummary}
            />
          </TabsContent>
        </Tabs>
      </div>
    </motion.div>
  );
}
