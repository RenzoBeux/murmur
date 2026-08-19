"use client";

import { useMemo, useState } from 'react';
import { Check, ChevronDown, Search, Settings2 } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
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

// Above this many models the model pane gets a search input (OpenRouter alone
// lists hundreds).
const SEARCH_THRESHOLD = 10;

interface ModelPickerProps {
  provider: ChatProvider;
  model: string;
  ollamaModels: string[];
  modelOptions: Record<string, string[]>;
  providerApiKeys: { claude: string | null; groq: string | null; openai: string | null; openrouter: string | null };
  chatgptSignedIn?: boolean;
  /** Called when the picker opens — lets the owner lazily fetch provider options. */
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
  const [open, setOpen] = useState(false);
  // The provider pane drives the model pane. Defaults to the provider in use.
  const [browseProvider, setBrowseProvider] = useState<ChatProvider>(provider);
  const [query, setQuery] = useState('');

  // Only providers that are usable right now are listed — no API key, no
  // sign-in, or no reachable models means the provider is hidden entirely; the
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

  // The browsed provider may not be in `groups` yet: options are probed lazily
  // on first open, so a cloud provider's list can arrive a moment later.
  const browsedGroup = groups.find((g) => g.provider === browseProvider);
  const showSearch = (browsedGroup?.models.length ?? 0) > SEARCH_THRESHOLD;

  const filteredModels = useMemo(() => {
    const models = browsedGroup?.models ?? [];
    const q = query.trim().toLowerCase();
    if (!q) return models;
    return models.filter((m) => m.toLowerCase().includes(q));
  }, [browsedGroup, query]);

  const handleOpenChange = (next: boolean) => {
    setOpen(next);
    if (next) {
      setBrowseProvider(provider);
      setQuery('');
      onOpen?.();
    }
  };

  const pick = (m: string) => {
    if (!browsedGroup) return;
    onPick(browsedGroup.provider, m);
    setOpen(false);
  };

  const label = model ? `${provider}/${model}` : 'Pick a model';

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogTrigger asChild>
        <Button variant="outline" size="sm" className="gap-1 text-xs font-normal">
          <span className="max-w-[180px] truncate">{label}</span>
          <ChevronDown className="h-3.5 w-3.5 opacity-60" />
        </Button>
      </DialogTrigger>
      <DialogContent className="max-w-2xl gap-0 p-0 overflow-hidden" aria-describedby={undefined}>
        <div className="px-4 py-3 border-b border-border">
          <DialogTitle className="text-sm font-semibold">Choose a model</DialogTitle>
        </div>

        {groups.length === 0 ? (
          <div className="px-4 py-10 text-center text-sm text-muted-foreground">
            No AI providers set up yet.
          </div>
        ) : (
          <div className="flex h-[400px] min-h-0">
            {/* Provider pane */}
            <div className="w-52 shrink-0 overflow-y-auto border-r border-border p-2 space-y-0.5">
              {groups.map((g) => {
                const isBrowsed = g.provider === browseProvider;
                const isCurrent = g.provider === provider;
                return (
                  <button
                    key={g.provider}
                    onClick={() => { setBrowseProvider(g.provider); setQuery(''); }}
                    className={cn(
                      'w-full flex items-center justify-between gap-2 rounded-md px-2.5 py-2 text-left text-sm transition-colors',
                      isBrowsed ? 'bg-accent text-foreground' : 'text-muted-foreground hover:bg-accent/60 hover:text-foreground',
                    )}
                  >
                    <span className="truncate">
                      {PROVIDER_LABEL[g.provider]}
                      {isCurrent && <span className="ml-1.5 inline-block h-1.5 w-1.5 rounded-full bg-brand align-middle" aria-label="current provider" />}
                    </span>
                    <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground/70">{g.models.length}</span>
                  </button>
                );
              })}
            </div>

            {/* Model pane */}
            <div className="flex min-w-0 flex-1 flex-col">
              {showSearch && (
                <div className="border-b border-border p-2">
                  <div className="relative">
                    <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
                    <Input
                      autoFocus
                      value={query}
                      onChange={(e) => setQuery(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter' && filteredModels.length > 0) {
                          e.preventDefault();
                          pick(filteredModels[0]);
                        }
                      }}
                      placeholder={`Search ${browsedGroup?.models.length ?? 0} models…`}
                      className="h-8 pl-8 text-sm"
                    />
                  </div>
                </div>
              )}
              <div className="min-h-0 flex-1 overflow-y-auto p-2">
                {!browsedGroup ? (
                  <div className="px-2 py-8 text-center text-sm text-muted-foreground">
                    No models available for this provider yet.
                  </div>
                ) : filteredModels.length === 0 ? (
                  <div className="px-2 py-8 text-center text-sm text-muted-foreground">
                    No models match “{query}”.
                  </div>
                ) : (
                  filteredModels.map((m) => {
                    const isActive = browsedGroup.provider === provider && m === model;
                    return (
                      <button
                        key={m}
                        onClick={() => pick(m)}
                        className={cn(
                          'w-full flex items-center justify-between gap-2 rounded-md px-2.5 py-1.5 text-left text-sm transition-colors hover:bg-accent',
                          isActive && 'bg-brand/10 text-brand',
                        )}
                      >
                        <span className="truncate">{m}</span>
                        {isActive && <Check className="h-3.5 w-3.5 shrink-0" />}
                      </button>
                    );
                  })
                )}
              </div>
            </div>
          </div>
        )}

        <div className="border-t border-border p-2">
          <button
            onClick={() => { setOpen(false); router.push('/settings?tab=aiProviders'); }}
            className="flex w-full items-center rounded-md px-2.5 py-1.5 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          >
            <Settings2 className="mr-2 h-4 w-4" />
            Manage providers…
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
