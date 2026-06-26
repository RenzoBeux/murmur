"use client";

import { Transcript, TranscriptSegmentData } from '@/types';
import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { TranscriptButtonGroup } from './TranscriptButtonGroup';
import { TranscriptEditorToolbar } from './TranscriptEditorToolbar';
import { useTranscriptEditor } from '@/hooks/meeting-details/useTranscriptEditor';
import { ExportScope } from '@/hooks/meeting-details/useExportOperations';
import { useEffect, useMemo } from 'react';

interface TranscriptPanelProps {
  transcripts: Transcript[];
  customPrompt: string;
  onPromptChange: (value: string) => void;
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  onExportMarkdown: (scope: ExportScope) => Promise<void>;
  hasSummary: boolean;
  isRecording: boolean;
  disableAutoScroll?: boolean;

  // Optional pagination props (when using virtualization)
  usePagination?: boolean;
  segments?: TranscriptSegmentData[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  loadedCount?: number;
  onLoadMore?: () => void;

  // Retranscription props
  meetingId?: string;
  meetingFolderPath?: string | null;
  onRefetchTranscripts?: () => Promise<void>;

  /**
   * Apply an in-memory mutation to the underlying transcript store. Required
   * to enable edit mode; when omitted, edit mode is disabled.
   */
  applyLocalMutation?: (mutator: (prev: Transcript[]) => Transcript[]) => void;
}

export function TranscriptPanel({
  transcripts,
  customPrompt,
  onPromptChange,
  onCopyTranscript,
  onOpenMeetingFolder,
  onExportMarkdown,
  hasSummary,
  isRecording,
  disableAutoScroll = false,
  usePagination = false,
  segments,
  hasMore,
  isLoadingMore,
  totalCount,
  loadedCount,
  onLoadMore,
  meetingId,
  meetingFolderPath,
  onRefetchTranscripts,
  applyLocalMutation,
}: TranscriptPanelProps) {
  // Convert transcripts to segments if pagination is not used but we want virtualization
  const convertedSegments = useMemo(() => {
    if (usePagination && segments) {
      return segments;
    }
    // Convert transcripts to segments for virtualization
    return transcripts.map(t => ({
      id: t.id,
      timestamp: t.audio_start_time ?? 0,
      endTime: t.audio_end_time,
      text: t.text,
      confidence: t.confidence,
      speaker: t.speaker,
    }));
  }, [transcripts, usePagination, segments]);

  const noopMutation = useMemo(
    () => (_mutator: (prev: Transcript[]) => Transcript[]) => {
      void _mutator;
    },
    [],
  );
  const editor = useTranscriptEditor({
    transcripts,
    applyLocalMutation: applyLocalMutation ?? noopMutation,
    meetingId,
  });
  // Edit mode is only available on saved meetings (not during live recording),
  // when there is at least one segment to operate on, and when the parent has
  // wired in a real applyLocalMutation (otherwise optimistic updates would
  // silently no-op).
  const canEnterEditMode = !isRecording && convertedSegments.length > 0 && !!applyLocalMutation;
  const isEditMode = editor.isEditMode && canEnterEditMode;

  const editModeProps = isEditMode
    ? {
        selectedIds: editor.selectedIds,
        editingId: editor.editingId,
        knownSpeakers: editor.knownSpeakers,
        onToggleSelect: editor.toggleSelect,
        onStartEdit: editor.startEdit,
        onCommitEdit: editor.editText,
        onCancelEdit: editor.cancelEdit,
        onReassignRowSpeaker: (id: string, speaker: string | null) =>
          editor.reassignSpeakers([id], speaker),
        onSplit: editor.splitSegment,
      }
    : undefined;

  const mergeValidation = isEditMode ? editor.validateMerge(Array.from(editor.selectedIds)) : null;
  const canMerge = mergeValidation?.ok ?? false;
  const mergeBlockedReason = mergeValidation && !mergeValidation.ok ? mergeValidation.reason : undefined;

  // Undo/redo keyboard shortcuts only fire while in edit mode and the user
  // isn't typing inside a regular input outside the transcript editor (the
  // inline textarea handles its own Ctrl+Enter for split; Ctrl+Z bubbles).
  useEffect(() => {
    if (!isEditMode) return;
    const handler = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const isFormField =
        target instanceof HTMLInputElement ||
        target instanceof HTMLSelectElement ||
        target instanceof HTMLTextAreaElement ||
        target?.isContentEditable;
      // Allow undo/redo even when an inline textarea is focused — Ctrl+Z on a
      // textarea's local input history is rare and the editor's history is
      // what the user actually wants.
      if ((e.ctrlKey || e.metaKey) && (e.key === 'z' || e.key === 'Z')) {
        if (e.shiftKey) {
          if (!editor.canRedo) return;
          e.preventDefault();
          void editor.redo();
        } else {
          if (!editor.canUndo) return;
          e.preventDefault();
          void editor.undo();
        }
        return;
      }
      if ((e.ctrlKey || e.metaKey) && (e.key === 'y' || e.key === 'Y')) {
        if (!editor.canRedo) return;
        e.preventDefault();
        void editor.redo();
        return;
      }
      // Suppress unused-var warning
      void isFormField;
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [isEditMode, editor.canUndo, editor.canRedo, editor.undo, editor.redo]);

  return (
    <div className="hidden md:flex md:w-1/4 lg:w-1/3 min-w-0 border-r border-gray-200 bg-white flex-col relative shrink-0 @container">
      {/* Title area */}
      <div className="p-4 border-b border-gray-200">
        <TranscriptButtonGroup
          transcriptCount={usePagination ? (totalCount ?? convertedSegments.length) : (transcripts?.length || 0)}
          onCopyTranscript={onCopyTranscript}
          onOpenMeetingFolder={onOpenMeetingFolder}
          onExportMarkdown={onExportMarkdown}
          hasSummary={hasSummary}
          meetingId={meetingId}
          meetingFolderPath={meetingFolderPath}
          onRefetchTranscripts={onRefetchTranscripts}
          isEditMode={isEditMode}
          onEnterEditMode={canEnterEditMode ? editor.enterEditMode : undefined}
          onExitEditMode={editor.exitEditMode}
          isRecording={isRecording}
          canUndo={editor.canUndo}
          canRedo={editor.canRedo}
          onUndo={editor.undo}
          onRedo={editor.redo}
        />
      </div>

      {isEditMode && (
        <TranscriptEditorToolbar
          selectionCount={editor.selectionCount}
          canMerge={canMerge}
          mergeBlockedReason={mergeBlockedReason}
          getMergeSpeakers={() => {
            const v = editor.validateMerge(Array.from(editor.selectedIds));
            return v.ok ? v.speakers : [];
          }}
          knownSpeakers={editor.knownSpeakers}
          onMerge={
            canMerge
              ? (speakerOverride) =>
                  editor.mergeSegments(Array.from(editor.selectedIds), speakerOverride)
              : undefined
          }
          onReassignSpeaker={
            editor.hasSelection
              ? (speaker) =>
                  editor.reassignSpeakers(Array.from(editor.selectedIds), speaker)
              : undefined
          }
          onDelete={editor.hasSelection ? editor.deleteSelected : undefined}
          onClear={editor.clearSelection}
        />
      )}

      {/* Transcript content - use virtualized view for better performance */}
      <div className="flex-1 overflow-hidden pb-4">
        <VirtualizedTranscriptView
          segments={convertedSegments}
          isRecording={isRecording}
          isPaused={false}
          isProcessing={false}
          isStopping={false}
          enableStreaming={false}
          showConfidence={true}
          disableAutoScroll={disableAutoScroll}
          editMode={editModeProps}
          hasMore={hasMore}
          isLoadingMore={isLoadingMore}
          totalCount={totalCount}
          loadedCount={loadedCount}
          onLoadMore={onLoadMore}
        />
      </div>

      {/* Custom prompt input at bottom of transcript section */}
      {!isRecording && !isEditMode && convertedSegments.length > 0 && (
        <div className="p-1 border-t border-gray-200">
          <textarea
            placeholder="Add context for AI summary. For example people involved, meeting overview, objective etc..."
            className="w-full px-3 py-2 border border-gray-200 rounded-md text-sm focus:outline-none focus:ring-1 focus:ring-blue-500 focus:border-blue-500 bg-white shadow-sm min-h-[80px] resize-y"
            value={customPrompt}
            onChange={(e) => onPromptChange(e.target.value)}
          />
        </div>
      )}
    </div>
  );
}
