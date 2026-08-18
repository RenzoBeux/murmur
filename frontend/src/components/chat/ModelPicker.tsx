"use client";

import { ChevronDown } from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { cn } from '@/lib/utils';

export type ChatProvider = 'ollama' | 'claude' | 'groq' | 'openai' | 'builtin-ai' | 'custom-openai' | 'openrouter' | 'lmstudio' | 'chatgpt-subscription';

export const PROVIDER_LABEL: Record<ChatProvider, string> = {
  ollama: 'Ollama (local)',
  claude: 'Claude',
  groq: 'Groq',
  openai: 'OpenAI',
  'builtin-ai': 'Built-in AI (local)',
  'custom-openai': 'Custom OpenAI',
  openrouter: 'OpenRouter',
  lmstudio: 'LM Studio (local)',
  'chatgpt-subscription': 'ChatGPT (subscription)',
};

interface ModelPickerProps {
  provider: ChatProvider;
  model: string;
  ollamaModels: string[];
  modelOptions: Record<string, string[]>;
  providerApiKeys: { claude: string | null; groq: string | null; openai: string | null; openrouter: string | null };
  onPick: (provider: ChatProvider, model: string) => void;
}

export function ModelPicker({
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
            <DropdownMenuLabel className="flex items-center justify-between text-xs uppercase tracking-wide text-muted-foreground">
              <span>{PROVIDER_LABEL[group.provider]}</span>
              {group.disabledReason && (
                <span className="text-[10px] text-warning normal-case tracking-normal">
                  {group.disabledReason}
                </span>
              )}
            </DropdownMenuLabel>
            {group.models.length === 0 ? (
              <div className="px-2 py-1.5 text-xs text-muted-foreground">No models available</div>
            ) : (
              group.models.map((m) => {
                const isActive = group.provider === provider && m === model;
                return (
                  <DropdownMenuItem
                    key={`${group.provider}-${m}`}
                    disabled={!!group.disabledReason}
                    onSelect={() => onPick(group.provider, m)}
                    className={cn('text-sm', isActive && 'bg-brand/10 text-brand')}
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
