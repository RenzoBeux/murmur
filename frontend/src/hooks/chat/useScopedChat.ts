import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { ChatMessage } from '@/types';
import { ChatScope, chatCommands, chatScopeArgs, chatScopeId, chatScopeKey } from '@/lib/chatScope';

interface UseScopedChatProps {
  scope: ChatScope;
  /** null = no thread yet; the panel creates one lazily on the first send. */
  threadId: string | null;
  provider: string;
  model: string;
}

/**
 * The messages of one chat thread, for any scope.
 *
 * Extracted verbatim from the meeting chat once the project chat became a
 * second consumer — `useMeetingChat` is now a thin wrapper over this, so the
 * shipped meeting chat exercises this code on every run and a regression here
 * cannot hide.
 */
export function useScopedChat({ scope, threadId, provider, model }: UseScopedChatProps) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [isLoadingHistory, setIsLoadingHistory] = useState(false);
  const [isSending, setIsSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const scopeKey = chatScopeKey(scope);
  const scopeId = chatScopeId(scope);
  const commands = chatCommands(scope);
  const scopeArgs = chatScopeArgs(scope);

  // Guards stale async results after switching scope or thread.
  const contextKeyRef = useRef(`${scopeKey}/${threadId ?? ''}`);
  contextKeyRef.current = `${scopeKey}/${threadId ?? ''}`;

  const loadHistory = useCallback(async () => {
    if (!scopeId || !threadId) {
      setMessages([]);
      return;
    }
    const contextKey = `${scopeKey}/${threadId}`;
    setIsLoadingHistory(true);
    setError(null);
    try {
      const history = await invoke<ChatMessage[]>(commands.getHistory, {
        ...scopeArgs,
        threadId,
      });
      if (contextKeyRef.current === contextKey) {
        setMessages(history ?? []);
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      console.error('Failed to load chat history:', err);
      setError(msg);
    } finally {
      setIsLoadingHistory(false);
    }
    // scopeKey stands in for the scope object, which is rebuilt each render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scopeKey, scopeId, threadId]);

  useEffect(() => {
    setMessages([]);
    void loadHistory();
  }, [loadHistory]);

  const sendMessage = useCallback(
    async (text: string, threadIdOverride?: string) => {
      const trimmed = text.trim();
      // The override lets a caller send into a thread it just created, before
      // the selectedThreadId state update has propagated.
      const targetThreadId = threadIdOverride ?? threadId;
      if (!trimmed || !scopeId || !targetThreadId || isSending) return;
      if (!provider || !model) {
        toast.error('Pick a model in the chat header before sending.');
        return;
      }

      const optimisticUser: ChatMessage = {
        id: `tmp-${Date.now()}`,
        meeting_id: scope.kind === 'meeting' ? scope.meetingId : null,
        project_id: scope.kind === 'project' ? scope.projectId : null,
        thread_id: targetThreadId,
        role: 'user',
        content: trimmed,
        created_at: new Date().toISOString(),
      };
      setMessages((prev) => [...prev, optimisticUser]);
      setIsSending(true);
      setError(null);

      const contextKey = `${scopeKey}/${targetThreadId}`;
      try {
        const reply = await invoke<ChatMessage>(commands.sendMessage, {
          ...scopeArgs,
          threadId: targetThreadId,
          message: trimmed,
          provider,
          model,
        });
        if (contextKeyRef.current === contextKey) {
          setMessages((prev) => [...prev, reply]);
        }
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error('Failed to send chat message:', err);
        setError(msg);
        toast.error(`Chat failed: ${msg}`);
        setMessages((prev) => prev.filter((m) => m.id !== optimisticUser.id));
      } finally {
        setIsSending(false);
      }
      // eslint-disable-next-line react-hooks/exhaustive-deps
    },
    [scopeKey, scopeId, threadId, isSending, provider, model]
  );

  const clearChat = useCallback(async () => {
    if (!scopeId || !threadId) return;
    try {
      await invoke(commands.clearHistory, { ...scopeArgs, threadId });
      setMessages([]);
      setError(null);
      toast.success('Chat cleared');
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      console.error('Failed to clear chat:', err);
      toast.error(`Failed to clear chat: ${msg}`);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scopeKey, scopeId, threadId]);

  return {
    messages,
    isLoadingHistory,
    isSending,
    error,
    sendMessage,
    clearChat,
    reloadHistory: loadHistory,
  };
}
