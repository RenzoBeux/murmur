import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { ChatAnswerMetadata, ChatGrounding, LiveChatMessage } from '@/types';
import { useRecordingState } from '@/contexts/RecordingStateContext';

interface UseLiveChatProps {
  provider: string;
  model: string;
  grounding: ChatGrounding;
}

/**
 * A live message with its metadata already parsed. The Rust side hands back the
 * raw JSON string it will persist, but the renderer wants the object.
 */
export type LiveDisplayMessage = Omit<LiveChatMessage, 'metadata'> & {
  metadata?: ChatAnswerMetadata;
};

function parseAnswerMetadata(raw?: string): ChatAnswerMetadata | undefined {
  if (!raw) return undefined;
  try {
    return JSON.parse(raw) as ChatAnswerMetadata;
  } catch (err) {
    // Losing a badge is not worth losing the answer.
    console.error('Ignoring unreadable live chat metadata:', err);
    return undefined;
  }
}

function toDisplayMessage(message: LiveChatMessage): LiveDisplayMessage {
  const { metadata, ...rest } = message;
  return { ...rest, metadata: parseAnswerMetadata(metadata) };
}

/**
 * The Ask-AI conversation during a live recording. The conversation of record
 * lives in the Rust in-memory store (it survives webview reloads and is
 * persisted as the meeting's "Live chat" thread at stop); this hook mirrors it
 * into React state and talks to the live chat commands.
 */
export function useLiveChat({ provider, model, grounding }: UseLiveChatProps) {
  const { isRecording } = useRecordingState();
  const [messages, setMessages] = useState<LiveDisplayMessage[]>([]);
  const [isSending, setIsSending] = useState(false);

  // Rehydrate whenever a recording is active — covers both a fresh start
  // (backend just cleared the store, so this loads an empty list) and a
  // webview reload mid-recording (this restores the conversation).
  useEffect(() => {
    if (!isRecording) return;
    let cancelled = false;
    invoke<LiveChatMessage[]>('api_get_live_chat_history')
      .then((history) => {
        if (!cancelled) setMessages((history ?? []).map(toDisplayMessage));
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

      const optimisticUser: LiveDisplayMessage = {
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
          grounding,
        });
        setMessages((prev) => [...prev, toDisplayMessage(reply)]);
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error('Failed to send live chat message:', err);
        toast.error(`Chat failed: ${msg}`);
        setMessages((prev) => prev.filter((m) => m.id !== optimisticUser.id));
      } finally {
        setIsSending(false);
      }
    },
    [isSending, isRecording, provider, model, grounding]
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
