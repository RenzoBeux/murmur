"use client";

import { useState } from 'react';
import { Sparkles, Trash2, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { ModelPicker } from '@/components/chat/ModelPicker';
import { ChatMessageList } from '@/components/chat/ChatMessageList';
import { ChatComposer } from '@/components/chat/ChatComposer';
import { useChatModelSelection } from '@/hooks/useChatModelSelection';
import { useLiveChat } from '@/hooks/useLiveChat';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { useTranscripts } from '@/contexts/TranscriptContext';

interface LiveChatPanelProps {
  onClose: () => void;
}

/**
 * Ask-AI side panel on the live recording screen: answers questions about the
 * meeting so far, grounded in the in-progress transcript. The conversation is
 * carried over to the saved meeting as its "Live chat" thread when the
 * recording stops.
 */
export function LiveChatPanel({ onClose }: LiveChatPanelProps) {
  const { provider, model, ollamaModelNames, modelOptions, providerApiKeys, handlePickModel } =
    useChatModelSelection();
  const { messages, isSending, sendMessage, clearChat } = useLiveChat({ provider, model });
  const { status, isRecording } = useRecordingState();
  const { transcripts } = useTranscripts();

  const [input, setInput] = useState('');

  const hasTranscript = transcripts.length > 0;
  const isSavingOrStopping =
    status === RecordingStatus.STOPPING ||
    status === RecordingStatus.PROCESSING_TRANSCRIPTS ||
    status === RecordingStatus.SAVING;

  const canAsk = isRecording && status === RecordingStatus.RECORDING && hasTranscript;
  const placeholder = !isRecording
    ? isSavingOrStopping
      ? 'Recording is being saved…'
      : 'Start recording to ask about it.'
    : !hasTranscript
      ? 'Waiting for the first words…'
      : 'Ask about the meeting so far…';

  const handleSend = () => {
    const text = input.trim();
    if (!text) return;
    setInput('');
    void sendMessage(text);
  };

  const handleClear = () => {
    if (messages.length === 0) return;
    const ok = window.confirm('Clear this conversation?');
    if (ok) void clearChat();
  };

  return (
    <div className="flex h-full flex-col bg-background">
      <div className="flex items-center justify-between border-b border-border px-3 py-3">
        <div className="flex items-center gap-2 text-sm font-medium text-muted-foreground">
          <Sparkles className="h-4 w-4 text-brand" />
          <span>Ask AI</span>
        </div>
        <div className="flex items-center gap-1">
          <ModelPicker
            provider={provider}
            model={model}
            ollamaModels={ollamaModelNames}
            modelOptions={modelOptions}
            providerApiKeys={providerApiKeys}
            onPick={handlePickModel}
          />
          <Button
            variant="ghost"
            size="icon"
            onClick={handleClear}
            disabled={messages.length === 0 || isSending}
            className="text-muted-foreground hover:text-destructive"
            aria-label="Clear conversation"
          >
            <Trash2 className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            onClick={onClose}
            aria-label="Close Ask AI panel"
          >
            <X className="h-4 w-4" />
          </Button>
        </div>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto px-3 py-4">
        {messages.length === 0 ? (
          <LiveEmptyState canAsk={canAsk} onUseSuggestion={(s) => setInput(s)} />
        ) : (
          <ChatMessageList messages={messages} isSending={isSending} />
        )}
      </div>

      <ChatComposer
        value={input}
        onChange={setInput}
        onSend={handleSend}
        placeholder={placeholder}
        disabled={!canAsk}
        sendDisabled={!model}
        isSending={isSending}
      />
    </div>
  );
}

const LIVE_SUGGESTIONS = [
  'What have we decided so far?',
  'Summarize the discussion so far.',
  'What questions are still open?',
];

function LiveEmptyState({
  canAsk,
  onUseSuggestion,
}: {
  canAsk: boolean;
  onUseSuggestion: (s: string) => void;
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 text-center">
      <div className="flex h-12 w-12 items-center justify-center rounded-full bg-brand/10">
        <Sparkles className="h-6 w-6 text-brand" />
      </div>
      <div className="max-w-sm text-sm text-muted-foreground">
        {canAsk
          ? 'Ask about the meeting while it happens. Answers are grounded in the transcript so far, and the conversation is saved with the meeting.'
          : 'Once the recording picks up the first words, you can ask questions about the meeting as it happens.'}
      </div>
      {canAsk && (
        <div className="flex flex-wrap justify-center gap-2">
          {LIVE_SUGGESTIONS.map((s) => (
            <button
              key={s}
              type="button"
              onClick={() => onUseSuggestion(s)}
              className="rounded-full border border-border bg-secondary px-3 py-1 text-xs text-muted-foreground hover:border-brand/40 hover:text-brand"
            >
              {s}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
