"use client";

import { ChevronDown, Settings2 } from 'lucide-react';
import { useRouter } from 'next/navigation';
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
  chatgptSignedIn?: boolean;
  /** Called when the dropdown opens — lets the owner lazily fetch provider options. */
  onOpen?: () => void;
  onPick: (provider: ChatProvider, model: string) => void;
}

export function ModelPicker({
  provider,
  model,
  ollamaModels,
  modelOptions,
  providerApiKeys,
  chatgptSignedIn,
  onOpen,
  onPick,
}: ModelPickerProps) {
  const router = useRouter();

  // Only providers that are usable right now are listed — no API key, no
  // sign-in, or no reachable models means the group is hidden entirely; the
  // "Manage providers" footer is the path to setting more up. Local providers
  // first, then cloud (mirroring providerLocality).
  const allGroups: Array<{ provider: ChatProvider; models: string[]; configured: boolean }> = [
    { provider: 'ollama', models: ollamaModels.length > 0 ? ollamaModels : modelOptions.ollama || [], configured: true },
    { provider: 'lmstudio', models: modelOptions.lmstudio || [], configured: true },
    { provider: 'builtin-ai', models: modelOptions['builtin-ai'] || [], configured: true },
    { provider: 'claude', models: modelOptions.claude || [], configured: !!providerApiKeys.claude },
    { provider: 'groq', models: modelOptions.groq || [], configured: !!providerApiKeys.groq },
    { provider: 'openai', models: modelOptions.openai || [], configured: !!providerApiKeys.openai },
    { provider: 'openrouter', models: modelOptions.openrouter || [], configured: !!providerApiKeys.openrouter },
    { provider: 'chatgpt-subscription', models: modelOptions['chatgpt-subscription'] || [], configured: !!chatgptSignedIn },
    { provider: 'custom-openai', models: modelOptions['custom-openai'] || [], configured: true },
  ];
  const groups = allGroups.filter((g) => g.configured && g.models.length > 0);

  const label = model ? `${provider}/${model}` : 'Pick a model';

  return (
    <DropdownMenu onOpenChange={(open) => { if (open) onOpen?.(); }}>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" size="sm" className="gap-1 text-xs font-normal">
          <span className="max-w-[180px] truncate">{label}</span>
          <ChevronDown className="h-3.5 w-3.5 opacity-60" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-64 max-h-96 overflow-y-auto">
        {groups.length === 0 && (
          <div className="px-2 py-1.5 text-xs text-muted-foreground">
            No AI providers set up yet.
          </div>
        )}
        {groups.map((group, idx) => (
          <div key={group.provider}>
            {idx > 0 && <DropdownMenuSeparator />}
            <DropdownMenuLabel className="text-xs uppercase tracking-wide text-muted-foreground">
              {PROVIDER_LABEL[group.provider]}
            </DropdownMenuLabel>
            {group.models.map((m) => {
              const isActive = group.provider === provider && m === model;
              return (
                <DropdownMenuItem
                  key={`${group.provider}-${m}`}
                  onSelect={() => onPick(group.provider, m)}
                  className={cn('text-sm', isActive && 'bg-brand/10 text-brand')}
                >
                  <span className="truncate">{m}</span>
                </DropdownMenuItem>
              );
            })}
          </div>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onSelect={() => router.push('/settings?tab=aiProviders')}
          className="text-sm text-muted-foreground"
        >
          <Settings2 className="mr-2 h-4 w-4" />
          Manage providers…
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
