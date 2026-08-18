"use client";

import { useState } from 'react';
import { ChevronDown, Loader2, MessageSquare, Plus, Sparkles, Trash2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { useMeetingChat } from '@/hooks/meeting-details/useMeetingChat';
import { useChatThreads } from '@/hooks/meeting-details/useChatThreads';
import { useChatModelSelection } from '@/hooks/useChatModelSelection';
import { ModelPicker } from '@/components/chat/ModelPicker';
import { ChatMessageList } from '@/components/chat/ChatMessageList';
import { ChatComposer } from '@/components/chat/ChatComposer';
import { ChatThread } from '@/types';
import { cn } from '@/lib/utils';

interface ChatPanelProps {
  meetingId: string;
  hasTranscripts: boolean;
}

export function ChatPanel({ meetingId, hasTranscripts }: ChatPanelProps) {
  const { provider, model, ollamaModelNames, modelOptions, providerApiKeys, handlePickModel } =
    useChatModelSelection();

  const {
    threads,
    selectedThreadId,
    setSelectedThreadId,
    createThread,
    deleteThread,
  } = useChatThreads(meetingId);

  const { messages, isLoadingHistory, isSending, sendMessage, clearChat } = useMeetingChat({
    meetingId,
    threadId: selectedThreadId,
    provider,
    model,
  });

  const [input, setInput] = useState('');

  const handleSend = async () => {
    const text = input.trim();
    if (!text || isSending) return;
    setInput('');
    if (selectedThreadId) {
      await sendMessage(text);
      return;
    }
    // Lazy thread creation: the first message of a meeting (or after deleting
    // every thread) creates its conversation on demand — no empty thread rows.
    const thread = await createThread();
    if (!thread) {
      setInput(text);
      return;
    }
    await sendMessage(text, thread.id);
  };

  const handleClear = async () => {
    if (messages.length === 0) return;
    const ok = window.confirm('Clear all messages in this chat?');
    if (ok) await clearChat();
  };

  const handleDeleteThread = async (thread: ChatThread) => {
    const ok = window.confirm(`Delete "${thread.title}" and all its messages?`);
    if (ok) await deleteThread(thread.id);
  };

  const selectedThread = threads.find((t) => t.id === selectedThreadId) ?? null;

  return (
    <div className="flex h-full flex-col bg-background">
      <div className="flex items-center justify-between border-b border-border px-4 py-3">
        <ThreadSwitcher
          threads={threads}
          selectedThread={selectedThread}
          onSelect={setSelectedThreadId}
          onNewThread={() => void createThread()}
          onDeleteThread={handleDeleteThread}
        />
        <div className="flex items-center gap-2">
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
            size="sm"
            onClick={handleClear}
            disabled={messages.length === 0 || isSending}
            className="text-muted-foreground hover:text-destructive"
          >
            <Trash2 className="h-4 w-4" />
            Clear
          </Button>
        </div>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto px-4 py-4">
        {isLoadingHistory ? (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            <Loader2 className="mr-2 h-4 w-4 animate-spin" /> Loading chat…
          </div>
        ) : messages.length === 0 ? (
          <EmptyState hasTranscripts={hasTranscripts} onUseSuggestion={(s) => setInput(s)} />
        ) : (
          <ChatMessageList messages={messages} isSending={isSending} />
        )}
      </div>

      <ChatComposer
        value={input}
        onChange={setInput}
        onSend={() => void handleSend()}
        placeholder={
          hasTranscripts
            ? 'Ask anything about this meeting…'
            : 'No transcript yet — record or import audio first.'
        }
        disabled={!hasTranscripts}
        sendDisabled={!model}
        isSending={isSending}
      />
    </div>
  );
}

function ThreadSwitcher({
  threads,
  selectedThread,
  onSelect,
  onNewThread,
  onDeleteThread,
}: {
  threads: ChatThread[];
  selectedThread: ChatThread | null;
  onSelect: (threadId: string) => void;
  onNewThread: () => void;
  onDeleteThread: (thread: ChatThread) => void;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="sm" className="gap-2 text-sm font-medium text-muted-foreground">
          <MessageSquare className="h-4 w-4 text-brand" />
          <span className="max-w-[160px] truncate">
            {selectedThread ? selectedThread.title : 'New chat'}
          </span>
          {selectedThread?.origin === 'live' && (
            <span className="rounded-full bg-brand/10 px-1.5 py-0.5 text-[10px] font-medium text-brand">
              live
            </span>
          )}
          <ChevronDown className="h-3.5 w-3.5 opacity-60" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-64 max-h-80 overflow-y-auto">
        <DropdownMenuItem onSelect={onNewThread} className="gap-2 text-sm">
          <Plus className="h-4 w-4" />
          New chat
        </DropdownMenuItem>
        {threads.length > 0 && <DropdownMenuSeparator />}
        {threads.map((thread) => {
          const isActive = thread.id === selectedThread?.id;
          return (
            <DropdownMenuItem
              key={thread.id}
              onSelect={() => onSelect(thread.id)}
              className={cn('group gap-2 text-sm', isActive && 'bg-brand/10 text-brand')}
            >
              <div className="flex min-w-0 flex-1 flex-col">
                <span className="flex items-center gap-1.5 truncate">
                  {thread.title}
                  {thread.origin === 'live' && (
                    <span className="rounded-full bg-brand/10 px-1.5 py-0.5 text-[10px] font-medium text-brand">
                      live
                    </span>
                  )}
                </span>
                <span className="text-[11px] text-muted-foreground">
                  {new Date(thread.created_at).toLocaleDateString()}
                </span>
              </div>
              <button
                type="button"
                aria-label={`Delete ${thread.title}`}
                className="rounded p-1 text-muted-foreground opacity-0 transition-opacity hover:text-destructive group-hover:opacity-100"
                onClick={(e) => {
                  e.stopPropagation();
                  e.preventDefault();
                  onDeleteThread(thread);
                }}
              >
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </DropdownMenuItem>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

const SUGGESTIONS = [
  'Summarize the action items.',
  'What decisions were made?',
  'Who was assigned what?',
  'What were the key disagreements?',
];

function EmptyState({
  hasTranscripts,
  onUseSuggestion,
}: {
  hasTranscripts: boolean;
  onUseSuggestion: (s: string) => void;
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 text-center">
      <div className="flex h-12 w-12 items-center justify-center rounded-full bg-brand/10">
        <Sparkles className="h-6 w-6 text-brand" />
      </div>
      <div className="max-w-sm text-sm text-muted-foreground">
        {hasTranscripts
          ? 'Ask follow-up questions about what was said. The assistant has access to the transcript and any attached files.'
          : 'Record or import a meeting first. Once a transcript exists, you can chat with it here.'}
      </div>
      {hasTranscripts && (
        <div className="flex flex-wrap justify-center gap-2">
          {SUGGESTIONS.map((s) => (
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
