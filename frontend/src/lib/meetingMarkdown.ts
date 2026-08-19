import { invoke as invokeTauri } from '@tauri-apps/api/core';
import { RefObject } from 'react';
import { Transcript, Summary } from '@/types';
import { BlockNoteSummaryViewRef } from '@/components/AISummary/BlockNoteSummaryView';
import { formatSpeaker } from '@/lib/speakerLabel';

export interface MeetingMarkdownContext {
  id: string;
  title: string;
  created_at: string;
}

export async function fetchAllTranscripts(meetingId: string): Promise<Transcript[]> {
  const firstPage = await invokeTauri('api_get_meeting_transcripts', {
    meetingId,
    limit: 1,
    offset: 0,
  }) as { transcripts: Transcript[]; total_count: number; has_more: boolean };

  if (firstPage.total_count === 0) {
    return [];
  }

  const allData = await invokeTauri('api_get_meeting_transcripts', {
    meetingId,
    limit: firstPage.total_count,
    offset: 0,
  }) as { transcripts: Transcript[]; total_count: number; has_more: boolean };

  return allData.transcripts;
}

function formatRecordingTimestamp(seconds: number | undefined, fallbackTimestamp: string): string {
  if (seconds === undefined) {
    return fallbackTimestamp;
  }
  const totalSecs = Math.floor(seconds);
  const mins = Math.floor(totalSecs / 60);
  const secs = totalSecs % 60;
  return `[${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}]`;
}

export function buildTranscriptMarkdown(
  meeting: MeetingMarkdownContext,
  meetingTitle: string,
  transcripts: Transcript[],
): string {
  const header = `# Transcript of the Meeting: ${meeting.id} - ${meetingTitle ?? meeting.title}\n\n`;
  const date = `## Date: ${new Date(meeting.created_at).toLocaleDateString()}\n\n`;
  const body = transcripts
    .map(t => {
      const speaker = formatSpeaker(t.speaker);
      const speakerPrefix = speaker ? ` **${speaker.label}:**` : '';
      return `${formatRecordingTimestamp(t.audio_start_time, t.timestamp)}${speakerPrefix} ${t.text}  `;
    })
    .join('\n');
  return header + date + body;
}

export async function buildSummaryMarkdown(
  meeting: MeetingMarkdownContext,
  meetingTitle: string,
  aiSummary: Summary | null,
  blockNoteSummaryRef: RefObject<BlockNoteSummaryViewRef>,
): Promise<string> {
  let summaryMarkdown = '';

  if (blockNoteSummaryRef.current?.getMarkdown) {
    summaryMarkdown = await blockNoteSummaryRef.current.getMarkdown();
  }

  if (!summaryMarkdown && aiSummary && 'markdown' in aiSummary) {
    summaryMarkdown = (aiSummary as any).markdown || '';
  }

  if (!summaryMarkdown && aiSummary) {
    const sections = Object.entries(aiSummary)
      .filter(([key]) => {
        return key !== 'markdown' && key !== 'summary_json' && key !== '_section_order' && key !== 'MeetingName';
      })
      .map(([, section]) => {
        if (section && typeof section === 'object' && 'title' in section && 'blocks' in section) {
          const sectionTitle = `## ${section.title}\n\n`;
          const sectionContent = section.blocks
            .map((block: any) => `- ${block.content}`)
            .join('\n');
          return sectionTitle + sectionContent;
        }
        return '';
      })
      .filter(s => s.trim())
      .join('\n\n');
    summaryMarkdown = sections;
  }

  if (!summaryMarkdown.trim()) {
    return '';
  }

  const dateOpts: Intl.DateTimeFormatOptions = {
    year: 'numeric',
    month: 'long',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  };

  const header = `# Meeting Summary: ${meetingTitle}\n\n`;
  const metadata =
    `**Meeting ID:** ${meeting.id}\n` +
    `**Date:** ${new Date(meeting.created_at).toLocaleDateString('en-US', dateOpts)}\n` +
    `**Generated on:** ${new Date().toLocaleDateString('en-US', dateOpts)}\n\n---\n\n`;

  return header + metadata + summaryMarkdown;
}
