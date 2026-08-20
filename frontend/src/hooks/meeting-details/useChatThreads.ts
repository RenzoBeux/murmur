import { useMemo } from 'react';
import { ChatScope } from '@/lib/chatScope';
import { useScopedChatThreads } from '@/hooks/chat/useScopedChatThreads';

/**
 * The chat threads (conversations) of a meeting. Threads are listed oldest
 * first; the newest one is auto-selected on load, so after a recording the
 * carried-over "Live chat" thread is what the Chat tab opens on.
 *
 * A thin wrapper over `useScopedChatThreads`, which the project chat shares.
 */
export function useChatThreads(meetingId: string) {
  const scope = useMemo<ChatScope>(() => ({ kind: 'meeting', meetingId }), [meetingId]);
  return useScopedChatThreads(scope);
}
