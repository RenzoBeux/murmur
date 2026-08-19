import { useCallback, RefObject } from 'react';
import { Transcript, Summary } from '@/types';
import { BlockNoteSummaryViewRef } from '@/components/AISummary/BlockNoteSummaryView';
import { toast } from 'sonner';
import {
  fetchAllTranscripts,
  buildTranscriptMarkdown,
  buildSummaryMarkdown,
} from '@/lib/meetingMarkdown';
import { writeTextToClipboard } from '@/lib/clipboard';

interface UseCopyOperationsProps {
  meeting: any;
  transcripts: Transcript[];
  meetingTitle: string;
  aiSummary: Summary | null;
  blockNoteSummaryRef: RefObject<BlockNoteSummaryViewRef>;
}

export function useCopyOperations({
  meeting,
  meetingTitle,
  aiSummary,
  blockNoteSummaryRef,
}: UseCopyOperationsProps) {

  const handleCopyTranscript = useCallback(async () => {
    let allTranscripts: Transcript[] = [];
    try {
      allTranscripts = await fetchAllTranscripts(meeting.id);
    } catch (error) {
      console.error('❌ Error fetching all transcripts:', error);
      toast.error('Failed to fetch transcripts for copying');
      return;
    }

    if (!allTranscripts.length) {
      toast.error('No transcripts available to copy');
      return;
    }

    const markdown = buildTranscriptMarkdown(meeting, meetingTitle, allTranscripts);
    try {
      await writeTextToClipboard(markdown);
      toast.success("Transcript copied to clipboard");
    } catch (error) {
      console.error('❌ Failed to copy transcript:', error);
      toast.error("Failed to copy transcript");
    }
  }, [meeting, meetingTitle]);

  const handleCopySummary = useCallback(async () => {
    try {
      const fullMarkdown = await buildSummaryMarkdown(meeting, meetingTitle, aiSummary, blockNoteSummaryRef);

      if (!fullMarkdown.trim()) {
        toast.error('No summary content available to copy');
        return;
      }

      await writeTextToClipboard(fullMarkdown);
      toast.success("Summary copied to clipboard");
    } catch (error) {
      console.error('❌ Failed to copy summary:', error);
      toast.error("Failed to copy summary");
    }
  }, [aiSummary, meetingTitle, meeting, blockNoteSummaryRef]);

  return {
    handleCopyTranscript,
    handleCopySummary,
  };
}
