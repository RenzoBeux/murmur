"use client";

import { FormEvent, KeyboardEvent, useEffect, useMemo, useRef, useState } from 'react';
import { ChevronDown, Loader2, MessageSquare, Send, Sparkles, Trash2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { useMeetingChat } from '@/hooks/meeting-details/useMeetingChat';
import { useConfig } from '@/contexts/ConfigContext';
import { ModelConfig } from '@/services/configService';
import { ChatMessage } from '@/types';
import { cn } from '@/lib/utils';
import { toast } from 'sonner';
import { invoke } from '@tauri-apps/api/core';

interface ChatPanelProps {
  meetingId: string;
  hasTranscripts: boolean;
}

type ChatProvider = 'ollama' | 'claude' | 'groq' | 'openai' | 'builtin-ai' | 'custom-openai' | 'openrouter';

const PROVIDER_LABEL: Record<ChatProvider, string> = {
  ollama: 'Ollama (local)',
  claude: 'Claude',
  groq: 'Groq',
  openai: 'OpenAI',
  'builtin-ai': 'Built-in AI (local)',
  'custom-openai': 'Custom OpenAI',
  openrouter: 'OpenRouter',
};

export function ChatPanel({ meetingId, hasTranscripts }: ChatPanelProps) {
  const { modelConfig, setModelConfig, models, modelOptions, providerApiKeys } = useConfig();

  const provider = (modelConfig.provider as ChatProvider) || 'ollama';
  const model = modelConfig.model || '';

  const { messages, isLoadingHistory, isSending, sendMessage, clearChat } = useMeetingChat({
    meetingId,
    provider,
    model,
  });

  const [input, setInput] = useState('');
  const scrollAnchorRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    scrollAnchorRef.current?.scrollIntoView({ behavior: 'smooth', block: 'end' });
  }, [messages.length, isSending]);

  const handleSubmit = async (e?: FormEvent) => {
    e?.preventDefault();
    const text = input.trim();
    if (!text || isSending) return;
    setInput('');
    await sendMessage(text);
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      void handleSubmit();
    }
  };

  const handleClear = async () => {
    if (messages.length === 0) return;
    const ok = window.confirm('Clear the entire chat history for this meeting?');
    if (ok) await clearChat();
  };

  const persistModelChange = async (next: ModelConfig) => {
    try {
      await invoke('api_save_model_config', {
        provider: next.provider,
        model: next.model,
        whisperModel: next.whisperModel,
        apiKey: next.apiKey ?? null,
        ollamaEndpoint: next.ollamaEndpoint ?? null,
      });
      const { emit } = await import('@tauri-apps/api/event');
      await emit('model-config-updated', next);
    } catch (err) {
      console.error('Failed to save model config:', err);
      toast.error('Failed to save model selection');
    }
  };

  const handlePickModel = async (nextProvider: ChatProvider, nextModel: string) => {
    const requiresKey: ChatProvider[] = ['claude', 'groq', 'openai', 'openrouter'];
    if (requiresKey.includes(nextProvider)) {
      const key = providerApiKeys[nextProvider as keyof typeof providerApiKeys];
      if (!key) {
        toast.error(`No API key for ${PROVIDER_LABEL[nextProvider]}. Add one in Settings first.`);
        return;
      }
    }
    const next: ModelConfig = {
      ...modelConfig,
      provider: nextProvider,
      model: nextModel,
    };
    setModelConfig(next);
    await persistModelChange(next);
  };

  const ollamaModelNames = useMemo(() => models.map((m) => m.name), [models]);

  return (
    <div className="flex h-full flex-col bg-white">
      <div className="flex items-center justify-between border-b border-gray-200 px-4 py-3">
        <div className="flex items-center gap-2 text-sm font-medium text-gray-700">
          <MessageSquare className="h-4 w-4 text-blue-500" />
          <span>Chat with this meeting</span>
        </div>
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
            className="text-gray-500 hover:text-red-600"
          >
            <Trash2 className="h-4 w-4" />
            Clear
          </Button>
        </div>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto px-4 py-4">
        {isLoadingHistory ? (
          <div className="flex h-full items-center justify-center text-sm text-gray-400">
            <Loader2 className="mr-2 h-4 w-4 animate-spin" /> Loading chat…
          </div>
        ) : messages.length === 0 ? (
          <EmptyState hasTranscripts={hasTranscripts} onUseSuggestion={(s) => setInput(s)} />
        ) : (
          <div className="flex flex-col gap-3">
            {messages.map((msg) => (
              <MessageBubble key={msg.id} message={msg} />
            ))}
            {isSending && (
              <div className="flex items-center gap-2 self-start rounded-lg bg-gray-100 px-3 py-2 text-sm text-gray-500">
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
                Thinking…
              </div>
            )}
            <div ref={scrollAnchorRef} />
          </div>
        )}
      </div>

      <form onSubmit={handleSubmit} className="border-t border-gray-200 bg-gray-50 p-3">
        <div className="flex items-end gap-2">
          <Textarea
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={
              hasTranscripts
                ? 'Ask anything about this meeting…'
                : 'No transcript yet — record or import audio first.'
            }
            disabled={!hasTranscripts || isSending}
            rows={2}
            className="flex-1 resize-none bg-white"
          />
          <Button
            type="submit"
            variant="blue"
            size="icon"
            disabled={!input.trim() || !hasTranscripts || isSending || !model}
            aria-label="Send message"
          >
            {isSending ? <Loader2 className="h-4 w-4 animate-spin" /> : <Send className="h-4 w-4" />}
          </Button>
        </div>
        <p className="mt-1 text-[11px] text-gray-400">Enter to send · Shift+Enter for newline</p>
      </form>
    </div>
  );
}

interface ModelPickerProps {
  provider: ChatProvider;
  model: string;
  ollamaModels: string[];
  modelOptions: Record<string, string[]>;
  providerApiKeys: { claude: string | null; groq: string | null; openai: string | null; openrouter: string | null };
  onPick: (provider: ChatProvider, model: string) => void;
}

function ModelPicker({
  provider,
  model,
  ollamaModels,
  modelOptions,
  providerApiKeys,
  onPick,
}: ModelPickerProps) {
  const groups: Array<{ provider: ChatProvider; models: string[]; disabledReason?: string }> = [
    { provider: 'ollama', models: ollamaModels.length > 0 ? ollamaModels : modelOptions.ollama || [] },
    {
      provider: 'claude',
      models: modelOptions.claude || [],
      disabledReason: providerApiKeys.claude ? undefined : 'API key required',
    },
    {
      provider: 'groq',
      models: modelOptions.groq || [],
      disabledReason: providerApiKeys.groq ? undefined : 'API key required',
    },
    {
      provider: 'openai',
      models: modelOptions.openai || [],
      disabledReason: providerApiKeys.openai ? undefined : 'API key required',
    },
    {
      provider: 'builtin-ai',
      models: modelOptions['builtin-ai'] || [],
      disabledReason: (modelOptions['builtin-ai'] || []).length === 0
        ? 'Download a model in Settings'
        : undefined,
    },
  ];

  const label = model ? `${provider}/${model}` : 'Pick a model';

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" size="sm" className="gap-1 text-xs font-normal">
          <span className="max-w-[180px] truncate">{label}</span>
          <ChevronDown className="h-3.5 w-3.5 opacity-60" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-64 max-h-96 overflow-y-auto">
        {groups.map((group, idx) => (
          <div key={group.provider}>
            {idx > 0 && <DropdownMenuSeparator />}
            <DropdownMenuLabel className="flex items-center justify-between text-xs uppercase tracking-wide text-gray-500">
              <span>{PROVIDER_LABEL[group.provider]}</span>
              {group.disabledReason && (
                <span className="text-[10px] text-amber-600 normal-case tracking-normal">
                  {group.disabledReason}
                </span>
              )}
            </DropdownMenuLabel>
            {group.models.length === 0 ? (
              <div className="px-2 py-1.5 text-xs text-gray-400">No models available</div>
            ) : (
              group.models.map((m) => {
                const isActive = group.provider === provider && m === model;
                return (
                  <DropdownMenuItem
                    key={`${group.provider}-${m}`}
                    disabled={!!group.disabledReason}
                    onSelect={() => onPick(group.provider, m)}
                    className={cn('text-sm', isActive && 'bg-blue-50 text-blue-700')}
                  >
                    <span className="truncate">{m}</span>
                  </DropdownMenuItem>
                );
              })
            )}
          </div>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function MessageBubble({ message }: { message: ChatMessage }) {
  const isUser = message.role === 'user';
  return (
    <div className={cn('flex w-full', isUser ? 'justify-end' : 'justify-start')}>
      <div
        className={cn(
          'max-w-[85%] whitespace-pre-wrap rounded-lg px-3 py-2 text-sm shadow-sm',
          isUser ? 'bg-blue-500 text-white' : 'bg-gray-100 text-gray-800'
        )}
      >
        {message.content}
      </div>
    </div>
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
      <div className="flex h-12 w-12 items-center justify-center rounded-full bg-blue-50">
        <Sparkles className="h-6 w-6 text-blue-500" />
      </div>
      <div className="max-w-sm text-sm text-gray-500">
        {hasTranscripts
          ? 'Ask follow-up questions about what was said. The assistant has access to the transcript and any generated summary.'
          : 'Record or import a meeting first. Once a transcript exists, you can chat with it here.'}
      </div>
      {hasTranscripts && (
        <div className="flex flex-wrap justify-center gap-2">
          {SUGGESTIONS.map((s) => (
            <button
              key={s}
              type="button"
              onClick={() => onUseSuggestion(s)}
              className="rounded-full border border-gray-200 bg-white px-3 py-1 text-xs text-gray-600 hover:border-blue-300 hover:text-blue-600"
            >
              {s}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
