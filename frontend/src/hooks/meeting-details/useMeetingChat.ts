import { useMemo } from 'react';
import { ChatScope } from '@/lib/chatScope';
import { useScopedChat } from '@/hooks/chat/useScopedChat';

interface UseMeetingChatProps {
  meetingId: string;
  /** null = no thread yet; the panel creates one lazily on the first send. */
  threadId: string | null;
  provider: string;
  model: string;
}

/**
 * The messages of one meeting chat thread.
 *
 * A thin wrapper over `useScopedChat`, which the project chat shares. Kept as
 * its own hook with an unchanged signature so meeting-chat call sites did not
 * have to move when the project chat arrived.
 */
export function useMeetingChat({ meetingId, threadId, provider, model }: UseMeetingChatProps) {
  const scope = useMemo<ChatScope>(() => ({ kind: 'meeting', meetingId }), [meetingId]);
  return useScopedChat({ scope, threadId, provider, model });
}
