import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { ChatGrounding, ChatThread } from '@/types';

/**
 * The chat threads (conversations) of a meeting. Threads are listed oldest
 * first; the newest one is auto-selected on load, so after a recording the
 * carried-over "Live chat" thread is what the Chat tab opens on.
 *
 * `selectedThreadId` may be null (meeting with no chats yet) — the panel then
 * creates a thread lazily on the first send, so no empty thread rows pile up.
 */
export function useChatThreads(meetingId: string) {
  const [threads, setThreads] = useState<ChatThread[]>([]);
  const [selectedThreadId, setSelectedThreadId] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);

  const meetingIdRef = useRef(meetingId);
  meetingIdRef.current = meetingId;

  const loadThreads = useCallback(async () => {
    if (!meetingId) return;
    setIsLoading(true);
    try {
      const list = await invoke<ChatThread[]>('api_list_chat_threads', { meetingId });
      if (meetingIdRef.current !== meetingId) return;
      setThreads(list ?? []);
      setSelectedThreadId((current) => {
        if (current && (list ?? []).some((t) => t.id === current)) return current;
        return list && list.length > 0 ? list[list.length - 1].id : null;
      });
    } catch (err) {
      console.error('Failed to list chat threads:', err);
    } finally {
      setIsLoading(false);
    }
  }, [meetingId]);

  useEffect(() => {
    setThreads([]);
    setSelectedThreadId(null);
    void loadThreads();
  }, [loadThreads]);

  const createThread = useCallback(async (): Promise<ChatThread | null> => {
    if (!meetingId) return null;
    try {
      const thread = await invoke<ChatThread>('api_create_chat_thread', { meetingId, title: null });
      setThreads((prev) => [...prev, thread]);
      setSelectedThreadId(thread.id);
      return thread;
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      console.error('Failed to create chat thread:', err);
      toast.error(`Failed to create chat: ${msg}`);
      return null;
    }
  }, [meetingId]);

  /**
   * Change how far past the transcript a conversation may reach. Optimistic:
   * the picker flips immediately and rolls back if the write fails, so a mode
   * the backend rejected never looks applied.
   */
  const setGrounding = useCallback(
    async (threadId: string, grounding: ChatGrounding) => {
      if (!meetingId) return;
      setThreads((prev) =>
        prev.map((t) => (t.id === threadId ? { ...t, grounding_mode: grounding } : t))
      );
      try {
        await invoke<ChatThread>('api_set_chat_thread_grounding', {
          meetingId,
          threadId,
          grounding,
        });
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error('Failed to update chat grounding:', err);
        toast.error(`Failed to change answer scope: ${msg}`);
        // Re-read rather than remembering the old value: this is also called
        // immediately after createThread, when the new row is not in `threads`
        // yet and there is no previous value to restore.
        void loadThreads();
      }
    },
    [meetingId, loadThreads]
  );

  const deleteThread = useCallback(
    async (threadId: string) => {
      if (!meetingId) return;
      try {
        await invoke('api_delete_chat_thread', { meetingId, threadId });
        const next = threads.filter((t) => t.id !== threadId);
        setThreads(next);
        setSelectedThreadId((current) =>
          current === threadId ? (next.length > 0 ? next[next.length - 1].id : null) : current
        );
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error('Failed to delete chat thread:', err);
        toast.error(`Failed to delete chat: ${msg}`);
      }
    },
    [meetingId, threads]
  );

  return {
    threads,
    selectedThreadId,
    setSelectedThreadId,
    isLoading,
    createThread,
    deleteThread,
    setGrounding,
    reloadThreads: loadThreads,
  };
}
