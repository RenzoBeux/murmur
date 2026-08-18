import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { LiveChatMessage } from '@/types';
import { useRecordingState } from '@/contexts/RecordingStateContext';

interface UseLiveChatProps {
  provider: string;
  model: string;
}

/**
 * The Ask-AI conversation during a live recording. The conversation of record
 * lives in the Rust in-memory store (it survives webview reloads and is
 * persisted as the meeting's "Live chat" thread at stop); this hook mirrors it
 * into React state and talks to the live chat commands.
 */
export function useLiveChat({ provider, model }: UseLiveChatProps) {
  const { isRecording } = useRecordingState();
  const [messages, setMessages] = useState<LiveChatMessage[]>([]);
  const [isSending, setIsSending] = useState(false);

  // Rehydrate whenever a recording is active — covers both a fresh start
  // (backend just cleared the store, so this loads an empty list) and a
  // webview reload mid-recording (this restores the conversation).
  useEffect(() => {
    if (!isRecording) return;
    let cancelled = false;
    invoke<LiveChatMessage[]>('api_get_live_chat_history')
      .then((history) => {
        if (!cancelled) setMessages(history ?? []);
      })
      .catch((err) => console.error('Failed to load live chat history:', err));
    return () => {
      cancelled = true;
    };
  }, [isRecording]);

  const sendMessage = useCallback(
    async (text: string) => {
      const trimmed = text.trim();
      if (!trimmed || isSending || !isRecording) return;
      if (!provider || !model) {
        toast.error('Pick a model in the chat header before sending.');
        return;
      }

      const optimisticUser: LiveChatMessage = {
        id: `tmp-${Date.now()}`,
        role: 'user',
        content: trimmed,
        created_at: new Date().toISOString(),
      };
      setMessages((prev) => [...prev, optimisticUser]);
      setIsSending(true);

      try {
        const reply = await invoke<LiveChatMessage>('api_send_live_chat_message', {
          message: trimmed,
          provider,
          model,
        });
        setMessages((prev) => [...prev, reply]);
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error('Failed to send live chat message:', err);
        toast.error(`Chat failed: ${msg}`);
        setMessages((prev) => prev.filter((m) => m.id !== optimisticUser.id));
      } finally {
        setIsSending(false);
      }
    },
    [isSending, isRecording, provider, model]
  );

  const clearChat = useCallback(async () => {
    try {
      await invoke('api_clear_live_chat_history');
      setMessages([]);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      console.error('Failed to clear live chat:', err);
      toast.error(`Failed to clear chat: ${msg}`);
    }
  }, []);

  return { messages, isSending, sendMessage, clearChat };
}
