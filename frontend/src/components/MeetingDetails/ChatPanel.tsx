"use client";

import { useMemo } from 'react';
import { ChatPanelShell } from '@/components/chat/ChatPanelShell';
import { ChatScope } from '@/lib/chatScope';

interface ChatPanelProps {
  meetingId: string;
  hasTranscripts: boolean;
}

const MEETING_SUGGESTIONS = [
  'Summarize the action items.',
  'What decisions were made?',
  'Who was assigned what?',
  'What were the key disagreements?',
];

/**
 * The saved-meeting Chat tab. Everything mechanical lives in `ChatPanelShell`,
 * which the project chat shares; this supplies the meeting-specific copy.
 */
export function ChatPanel({ meetingId, hasTranscripts }: ChatPanelProps) {
  const scope = useMemo<ChatScope>(() => ({ kind: 'meeting', meetingId }), [meetingId]);

  return (
    <ChatPanelShell
      scope={scope}
      canChat={hasTranscripts}
      groundingScope="meeting"
      placeholder={
        hasTranscripts
          ? 'Ask anything about this meeting…'
          : 'No transcript yet — record or import audio first.'
      }
      emptyState={{
        body: hasTranscripts
          ? 'Ask follow-up questions about what was said. The assistant has access to the transcript and any attached files.'
          : 'Record or import a meeting first. Once a transcript exists, you can chat with it here.',
        suggestions: MEETING_SUGGESTIONS,
      }}
    />
  );
}
