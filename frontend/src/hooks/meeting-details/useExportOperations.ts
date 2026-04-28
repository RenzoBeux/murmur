import { useCallback, RefObject } from 'react';
import { Transcript, Summary } from '@/types';
import { BlockNoteSummaryViewRef } from '@/components/AISummary/BlockNoteSummaryView';
import { toast } from 'sonner';
import { invoke as invokeTauri } from '@tauri-apps/api/core';
import Analytics from '@/lib/analytics';
import {
  fetchAllTranscripts,
  buildTranscriptMarkdown,
  buildSummaryMarkdown,
  buildCombinedMarkdown,
  slugifyMeetingFilename,
} from '@/lib/meetingMarkdown';

export type ExportScope = 'transcript' | 'summary' | 'both';

interface UseExportOperationsProps {
  meeting: any;
  meetingTitle: string;
  aiSummary: Summary | null;
  blockNoteSummaryRef: RefObject<BlockNoteSummaryViewRef>;
}

export function useExportOperations({
  meeting,
  meetingTitle,
  aiSummary,
  blockNoteSummaryRef,
}: UseExportOperationsProps) {

  const handleExportMarkdown = useCallback(async (scope: ExportScope) => {
    try {
      Analytics.trackButtonClick(`export_markdown_${scope}`, 'meeting_details');

      let allTranscripts: Transcript[] = [];
      if (scope === 'transcript' || scope === 'both') {
        try {
          allTranscripts = await fetchAllTranscripts(meeting.id);
        } catch (error) {
          console.error('❌ Error fetching transcripts for export:', error);
          toast.error('Failed to fetch transcripts for export');
          return;
        }

        if (scope === 'transcript' && allTranscripts.length === 0) {
          toast.error('No transcripts available to export');
          return;
        }
      }

      let content = '';
      if (scope === 'transcript') {
        content = buildTranscriptMarkdown(meeting, meetingTitle, allTranscripts);
      } else if (scope === 'summary') {
        content = await buildSummaryMarkdown(meeting, meetingTitle, aiSummary, blockNoteSummaryRef);
        if (!content.trim()) {
          toast.error('No summary content available to export');
          return;
        }
      } else {
        content = await buildCombinedMarkdown(meeting, meetingTitle, allTranscripts, aiSummary, blockNoteSummaryRef);
      }

      const suggestedFilename = slugifyMeetingFilename(meetingTitle || meeting.title, meeting.created_at, scope);

      const savedPath = await invokeTauri<string | null>('export_meeting_markdown', {
        content,
        suggestedFilename,
      });

      if (savedPath) {
        toast.success(`Exported to ${savedPath}`);
      }
    } catch (error) {
      console.error('❌ Export failed:', error);
      toast.error(typeof error === 'string' ? error : 'Failed to export meeting');
    }
  }, [meeting, meetingTitle, aiSummary, blockNoteSummaryRef]);

  return {
    handleExportMarkdown,
  };
}
