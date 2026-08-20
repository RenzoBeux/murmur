import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { ChatGrounding, ChatThread } from '@/types';
import { ChatScope, chatCommands, chatScopeArgs, chatScopeId, chatScopeKey } from '@/lib/chatScope';

/**
 * The chat threads (conversations) of one scope. Threads are listed oldest
 * first; the newest one is auto-selected on load.
 *
 * `selectedThreadId` may be null (nothing chatted about yet) — the panel then
 * creates a thread lazily on the first send, so no empty thread rows pile up.
 */
export function useScopedChatThreads(scope: ChatScope) {
  const [threads, setThreads] = useState<ChatThread[]>([]);
  const [selectedThreadId, setSelectedThreadId] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);

  const scopeKey = chatScopeKey(scope);
  const scopeId = chatScopeId(scope);
  const commands = chatCommands(scope);
  const scopeArgs = chatScopeArgs(scope);

  const scopeKeyRef = useRef(scopeKey);
  scopeKeyRef.current = scopeKey;

  const loadThreads = useCallback(async () => {
    if (!scopeId) return;
    setIsLoading(true);
    try {
      const list = await invoke<ChatThread[]>(commands.listThreads, scopeArgs);
      if (scopeKeyRef.current !== scopeKey) return;
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
    // scopeKey stands in for the scope object, which is rebuilt each render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scopeKey, scopeId]);

  useEffect(() => {
    setThreads([]);
    setSelectedThreadId(null);
    void loadThreads();
  }, [loadThreads]);

  const createThread = useCallback(async (): Promise<ChatThread | null> => {
    if (!scopeId) return null;
    try {
      const thread = await invoke<ChatThread>(commands.createThread, {
        ...scopeArgs,
        title: null,
      });
      setThreads((prev) => [...prev, thread]);
      setSelectedThreadId(thread.id);
      return thread;
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      console.error('Failed to create chat thread:', err);
      toast.error(`Failed to create chat: ${msg}`);
      return null;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scopeKey, scopeId]);

  /**
   * Change how far past the source material a conversation may reach.
   * Optimistic: the picker flips immediately and rolls back if the write fails,
   * so a mode the backend rejected never looks applied.
   */
  const setGrounding = useCallback(
    async (threadId: string, grounding: ChatGrounding) => {
      if (!scopeId) return;
      setThreads((prev) =>
        prev.map((t) => (t.id === threadId ? { ...t, grounding_mode: grounding } : t))
      );
      try {
        await invoke<ChatThread>(commands.setThreadGrounding, {
          ...scopeArgs,
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
      // eslint-disable-next-line react-hooks/exhaustive-deps
    },
    [scopeKey, scopeId, loadThreads]
  );

  const deleteThread = useCallback(
    async (threadId: string) => {
      if (!scopeId) return;
      try {
        await invoke(commands.deleteThread, { ...scopeArgs, threadId });
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
      // eslint-disable-next-line react-hooks/exhaustive-deps
    },
    [scopeKey, scopeId, threads]
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
