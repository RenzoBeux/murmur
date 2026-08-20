"use client";

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Globe, Lightbulb, Lock } from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { ChatGrounding } from '@/types';
import { cn } from '@/lib/utils';

// Whether the selected provider/model can search the web. The table itself
// lives in Rust (summary/web_search.rs) so there is exactly one definition of
// it — this is only the shape it comes back in.
interface WebSearchSupportInfo {
  supported: boolean;
  reason?: string;
}

interface GroundingOption {
  value: ChatGrounding;
  label: string;
  hint: string;
  icon: typeof Lock;
}

/** What the strict mode is grounded in, which differs by chat scope. */
export type GroundingScope = 'meeting' | 'project';

// Ordered from strictest to broadest, which is also the order they appear in
// the menu — the default is first. The copy names the actual source material:
// "this meeting" is wrong, and quietly misleading, on a project chat.
const GROUNDING_OPTIONS: Record<GroundingScope, GroundingOption[]> = {
  meeting: [
    {
      value: 'transcript_only',
      label: 'Transcript only',
      hint: 'Answers strictly from this meeting. Says so when it cannot find something.',
      icon: Lock,
    },
    {
      value: 'general_knowledge',
      label: '+ General knowledge',
      hint: 'Can also explain things the meeting never covered. Stays offline.',
      icon: Lightbulb,
    },
    {
      value: 'web_search',
      label: '+ Web search',
      hint: 'Can also search the web and cite sources when the meeting falls short.',
      icon: Globe,
    },
  ],
  project: [
    {
      value: 'transcript_only',
      label: 'These meetings only',
      hint: "Answers strictly from this project's meetings. Says so when it cannot find something.",
      icon: Lock,
    },
    {
      value: 'general_knowledge',
      label: '+ General knowledge',
      hint: 'Can also explain things the meetings never covered. Stays offline.',
      icon: Lightbulb,
    },
    {
      value: 'web_search',
      label: '+ Web search',
      hint: 'Can also search the web and cite sources when the meetings fall short.',
      icon: Globe,
    },
  ],
};

export function groundingOption(
  value: ChatGrounding,
  scope: GroundingScope = 'meeting'
): GroundingOption {
  const options = GROUNDING_OPTIONS[scope];
  return options.find((o) => o.value === value) ?? options[0];
}

interface GroundingPickerProps {
  value: ChatGrounding;
  onChange: (value: ChatGrounding) => void;
  provider: string;
  model: string;
  disabled?: boolean;
  /** Selects the wording for the strict mode. Defaults to the meeting chat. */
  scope?: GroundingScope;
}

/**
 * Picks how far past the transcript the assistant may reach.
 *
 * Sits in the composer rather than the panel header because it changes what the
 * *next* question is allowed to do, so it belongs next to where that question is
 * typed. Web search is offered only when the selected provider can actually do
 * it; otherwise it is disabled with the reason, instead of being selectable and
 * quietly doing nothing.
 */
export function GroundingPicker({
  value,
  onChange,
  provider,
  model,
  disabled,
  scope = 'meeting',
}: GroundingPickerProps) {
  const [webSupport, setWebSupport] = useState<WebSearchSupportInfo | null>(null);

  useEffect(() => {
    if (!provider || !model) {
      setWebSupport(null);
      return;
    }
    let cancelled = false;
    invoke<WebSearchSupportInfo>('api_chat_web_search_support', { provider, model })
      .then((support) => {
        if (!cancelled) setWebSupport(support);
      })
      .catch((err) => {
        // Capability is advisory: on failure leave the option enabled and let
        // the backend degrade the answer, which it reports either way.
        console.error('Failed to check web search support:', err);
        if (!cancelled) setWebSupport(null);
      });
    return () => {
      cancelled = true;
    };
  }, [provider, model]);

  const isUnavailable = useCallback(
    (option: ChatGrounding) =>
      option === 'web_search' && webSupport !== null && !webSupport.supported,
    [webSupport]
  );

  const active = groundingOption(value, scope);
  const ActiveIcon = active.icon;
  // A thread can already be in web mode when the user switches to a model that
  // cannot search. Say so on the trigger rather than letting it look active.
  const activeDegraded = isUnavailable(value);

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={disabled}
          className={cn(
            'h-7 gap-1.5 px-2 text-[11px] font-medium text-muted-foreground hover:text-foreground',
            activeDegraded && 'text-amber-600 dark:text-amber-500'
          )}
          aria-label={`Answer scope: ${active.label}`}
        >
          <ActiveIcon className="h-3.5 w-3.5" />
          <span>{active.label}</span>
        </Button>
      </DropdownMenuTrigger>

      <DropdownMenuContent align="start" className="w-72">
        <DropdownMenuLabel className="text-[11px] font-normal text-muted-foreground">
          How far can the AI look?
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        {GROUNDING_OPTIONS[scope].map((option) => {
          const Icon = option.icon;
          const unavailable = isUnavailable(option.value);
          return (
            <DropdownMenuItem
              key={option.value}
              disabled={unavailable}
              onSelect={() => onChange(option.value)}
              className={cn(
                'flex items-start gap-2.5 py-2',
                option.value === value && 'bg-accent'
              )}
            >
              <Icon className="mt-0.5 h-4 w-4 shrink-0" />
              <div className="min-w-0">
                <div className="text-xs font-medium">{option.label}</div>
                <div className="text-[11px] leading-snug text-muted-foreground">
                  {unavailable ? webSupport?.reason : option.hint}
                </div>
              </div>
            </DropdownMenuItem>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
