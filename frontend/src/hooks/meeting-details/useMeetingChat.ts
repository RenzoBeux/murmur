import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { ChatMessage } from '@/types';

interface UseMeetingChatProps {
  meetingId: string;
  /** null = no thread yet; the panel creates one lazily on the first send. */
  threadId: string | null;
  provider: string;
  model: string;
}

export function useMeetingChat({ meetingId, threadId, provider, model }: UseMeetingChatProps) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [isLoadingHistory, setIsLoadingHistory] = useState(false);
  const [isSending, setIsSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Guards stale async results after switching meeting or thread.
  const contextKeyRef = useRef(`${meetingId}/${threadId ?? ''}`);
  contextKeyRef.current = `${meetingId}/${threadId ?? ''}`;

  const loadHistory = useCallback(async () => {
    if (!meetingId || !threadId) {
      setMessages([]);
      return;
    }
    const contextKey = `${meetingId}/${threadId}`;
    setIsLoadingHistory(true);
    setError(null);
    try {
      const history = await invoke<ChatMessage[]>('api_get_chat_history', { meetingId, threadId });
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
  }, [meetingId, threadId]);

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
      if (!trimmed || !meetingId || !targetThreadId || isSending) return;
      if (!provider || !model) {
        toast.error('Pick a model in the chat header before sending.');
        return;
      }

      const optimisticUser: ChatMessage = {
        id: `tmp-${Date.now()}`,
        meeting_id: meetingId,
        thread_id: targetThreadId,
        role: 'user',
        content: trimmed,
        created_at: new Date().toISOString(),
      };
      setMessages((prev) => [...prev, optimisticUser]);
      setIsSending(true);
      setError(null);

      const contextKey = `${meetingId}/${targetThreadId}`;
      try {
        const reply = await invoke<ChatMessage>('api_send_chat_message', {
          meetingId,
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
    },
    [meetingId, threadId, isSending, provider, model]
  );

  const clearChat = useCallback(async () => {
    if (!meetingId || !threadId) return;
    try {
      await invoke('api_clear_chat_history', { meetingId, threadId });
      setMessages([]);
      setError(null);
      toast.success('Chat cleared');
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      console.error('Failed to clear chat:', err);
      toast.error(`Failed to clear chat: ${msg}`);
    }
  }, [meetingId, threadId]);

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
