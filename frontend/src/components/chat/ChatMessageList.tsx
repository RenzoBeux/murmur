"use client";

import { useEffect, useRef } from 'react';
import { Loader2, Globe, Lightbulb, ExternalLink, AlertTriangle } from 'lucide-react';

import { ChatAnswerMetadata } from '@/types';
import { MarkdownContent } from '@/components/markdown/MarkdownContent';

// The minimal shape both chat surfaces (saved-meeting threads and the live
// Ask-AI panel) can render — their full message types both satisfy it.
export interface DisplayChatMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  /**
   * Where an assistant answer came from. Absent on user messages, on plain
   * transcript-only answers, and on anything written before grounding modes —
   * so an undecorated bubble means "answered from the meeting", same as always.
   */
  metadata?: ChatAnswerMetadata;
}

interface ChatMessageListProps {
  messages: DisplayChatMessage[];
  isSending: boolean;
}

export function ChatMessageList({ messages, isSending }: ChatMessageListProps) {
  const scrollAnchorRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    scrollAnchorRef.current?.scrollIntoView({ behavior: 'smooth', block: 'end' });
  }, [messages.length, isSending]);

  return (
    <div className="flex flex-col gap-3">
      {messages.map((msg) => (
        <MessageBubble key={msg.id} message={msg} />
      ))}
      {isSending && (
        <div className="flex items-center gap-2 self-start rounded-lg bg-muted px-3 py-2 text-sm text-muted-foreground">
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
          Thinking…
        </div>
      )}
      <div ref={scrollAnchorRef} />
    </div>
  );
}

function MessageBubble({ message }: { message: DisplayChatMessage }) {
  const isUser = message.role === 'user';

  // User messages are short, plain text — keep them as compact right-aligned
  // bubbles. Assistant replies are markdown and can be long/structured, so they
  // span the full column width and render as rich markdown.
  if (isUser) {
    return (
      <div className="flex w-full justify-end">
        <div className="max-w-[85%] whitespace-pre-wrap rounded-lg bg-primary px-3 py-2 text-sm text-primary-foreground shadow-sm">
          {message.content}
        </div>
      </div>
    );
  }

  return (
    <div className="flex w-full justify-start">
      <div className="w-full rounded-lg bg-card border border-border px-3 py-2 text-sm text-foreground shadow-sm">
        <AnswerScopeBadge metadata={message.metadata} />
        <MarkdownContent content={message.content} />
        <AnswerSources metadata={message.metadata} />
      </div>
    </div>
  );
}

/**
 * Says where an answer came from when it reached past the meeting, and flags the
 * case where the requested scope could not be honoured — a thread set to web
 * search but answered by a model that cannot search must not look like it
 * searched and found nothing.
 */
function AnswerScopeBadge({ metadata }: { metadata?: ChatAnswerMetadata }) {
  if (!metadata) return null;
  const { effective, degraded_reason: degradedReason } = metadata.grounding;
  if (effective === 'transcript_only' && !degradedReason) return null;

  const Icon = effective === 'web_search' ? Globe : Lightbulb;
  const searchCount = metadata.search_count ?? 0;
  const sourceCount = metadata.sources?.length ?? 0;

  // Only some providers report how many searches ran, and the model may decide
  // the meeting already answered the question. Say which of those happened
  // rather than implying a search that never took place.
  let label: string;
  if (effective !== 'web_search') {
    label = 'Includes general knowledge';
  } else if (searchCount > 0) {
    label = `Searched the web · ${searchCount} ${searchCount === 1 ? 'search' : 'searches'}`;
  } else if (sourceCount > 0) {
    label = 'Searched the web';
  } else {
    label = 'Answered without searching';
  }

  return (
    <div className="mb-1.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-[11px] text-muted-foreground">
      <span className="inline-flex items-center gap-1">
        <Icon className="h-3 w-3" />
        {label}
      </span>
      {degradedReason && (
        <span
          className="inline-flex items-center gap-1 text-amber-600 dark:text-amber-500"
          title={degradedReason}
        >
          <AlertTriangle className="h-3 w-3" />
          Web search unavailable
        </span>
      )}
    </div>
  );
}

/** Cited pages, as chips under the answer. */
function AnswerSources({ metadata }: { metadata?: ChatAnswerMetadata }) {
  const sources = metadata?.sources ?? [];
  if (sources.length === 0) return null;

  return (
    <div className="mt-2 flex flex-wrap gap-1.5 border-t border-border pt-2">
      {sources.map((source, index) => (
        <a
          key={`${source.url}-${index}`}
          href={source.url}
          target="_blank"
          rel="noreferrer noopener"
          title={source.cited_text ?? source.url}
          className="inline-flex max-w-[15rem] items-center gap-1 rounded-full border border-border bg-muted px-2 py-0.5 text-[11px] text-muted-foreground hover:text-foreground hover:border-foreground/30"
        >
          <ExternalLink className="h-3 w-3 shrink-0" />
          <span className="truncate">{sourceLabel(source.url, source.title)}</span>
        </a>
      ))}
    </div>
  );
}

/** Chips are narrow, so prefer the hostname over a long page title. */
function sourceLabel(url: string, title?: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, '');
  } catch {
    return title ?? url;
  }
}

