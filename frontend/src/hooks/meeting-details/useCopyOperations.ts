import { useCallback, RefObject } from 'react';
import { Transcript, Summary } from '@/types';
import { BlockNoteSummaryViewRef } from '@/components/AISummary/BlockNoteSummaryView';
import { toast } from 'sonner';
import Analytics from '@/lib/analytics';
import {
  fetchAllTranscripts,
  buildTranscriptMarkdown,
  buildSummaryMarkdown,
} from '@/lib/meetingMarkdown';

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
    await navigator.clipboard.writeText(markdown);
    toast.success("Transcript copied to clipboard");

    const wordCount = allTranscripts
      .map(t => t.text.split(/\s+/).length)
      .reduce((a, b) => a + b, 0);

    await Analytics.trackCopy('transcript', {
      meeting_id: meeting.id,
      transcript_length: allTranscripts.length.toString(),
      word_count: wordCount.toString()
    });
  }, [meeting, meetingTitle]);

  const handleCopySummary = useCallback(async () => {
    try {
      const fullMarkdown = await buildSummaryMarkdown(meeting, meetingTitle, aiSummary, blockNoteSummaryRef);

      if (!fullMarkdown.trim()) {
        toast.error('No summary content available to copy');
        return;
      }

      await navigator.clipboard.writeText(fullMarkdown);
      toast.success("Summary copied to clipboard");

      await Analytics.trackCopy('summary', {
        meeting_id: meeting.id,
        has_markdown: (!!aiSummary && 'markdown' in aiSummary).toString()
      });
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
