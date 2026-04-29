import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { ChatMessage } from '@/types';

interface UseMeetingChatProps {
  meetingId: string;
  provider: string;
  model: string;
}

export function useMeetingChat({ meetingId, provider, model }: UseMeetingChatProps) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [isLoadingHistory, setIsLoadingHistory] = useState(false);
  const [isSending, setIsSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const meetingIdRef = useRef(meetingId);
  meetingIdRef.current = meetingId;

  const loadHistory = useCallback(async () => {
    if (!meetingId) return;
    setIsLoadingHistory(true);
    setError(null);
    try {
      const history = await invoke<ChatMessage[]>('api_get_chat_history', { meetingId });
      if (meetingIdRef.current === meetingId) {
        setMessages(history ?? []);
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      console.error('Failed to load chat history:', err);
      setError(msg);
    } finally {
      setIsLoadingHistory(false);
    }
  }, [meetingId]);

  useEffect(() => {
    setMessages([]);
    void loadHistory();
  }, [loadHistory]);

  const sendMessage = useCallback(
    async (text: string) => {
      const trimmed = text.trim();
      if (!trimmed || !meetingId || isSending) return;
      if (!provider || !model) {
        toast.error('Pick a model in the chat header before sending.');
        return;
      }

      const optimisticUser: ChatMessage = {
        id: `tmp-${Date.now()}`,
        meeting_id: meetingId,
        role: 'user',
        content: trimmed,
        created_at: new Date().toISOString(),
      };
      setMessages((prev) => [...prev, optimisticUser]);
      setIsSending(true);
      setError(null);

      try {
        const reply = await invoke<ChatMessage>('api_send_chat_message', {
          meetingId,
          message: trimmed,
          provider,
          model,
        });
        if (meetingIdRef.current === meetingId) {
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
    [meetingId, isSending, provider, model]
  );

  const clearChat = useCallback(async () => {
    if (!meetingId) return;
    try {
      await invoke('api_clear_chat_history', { meetingId });
      setMessages([]);
      setError(null);
      toast.success('Chat cleared');
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      console.error('Failed to clear chat:', err);
      toast.error(`Failed to clear chat: ${msg}`);
    }
  }, [meetingId]);

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
