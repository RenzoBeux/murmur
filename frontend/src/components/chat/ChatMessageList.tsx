"use client";

import { useEffect, useRef } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Loader2 } from 'lucide-react';

// The minimal shape both chat surfaces (saved-meeting threads and the live
// Ask-AI panel) can render — their full message types both satisfy it.
export interface DisplayChatMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
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
        <MarkdownContent content={message.content} />
      </div>
    </div>
  );
}

// Renders assistant markdown with explicit utility classes rather than the
// `prose` (typography) plugin, for tighter control over chat-bubble spacing.
const MARKDOWN_COMPONENTS: Parameters<typeof ReactMarkdown>[0]['components'] = {
  p: ({ children }) => <p className="mb-2 last:mb-0 leading-relaxed">{children}</p>,
  ul: ({ children }) => <ul className="mb-2 last:mb-0 list-disc space-y-1 pl-5">{children}</ul>,
  ol: ({ children }) => <ol className="mb-2 last:mb-0 list-decimal space-y-1 pl-5">{children}</ol>,
  li: ({ children }) => <li className="marker:text-muted-foreground">{children}</li>,
  h1: ({ children }) => <h1 className="mb-2 mt-1 text-base font-semibold">{children}</h1>,
  h2: ({ children }) => <h2 className="mb-2 mt-1 text-sm font-semibold">{children}</h2>,
  h3: ({ children }) => <h3 className="mb-1 mt-1 text-sm font-semibold">{children}</h3>,
  strong: ({ children }) => <strong className="font-semibold">{children}</strong>,
  em: ({ children }) => <em className="italic">{children}</em>,
  a: ({ href, children }) => (
    <a href={href} target="_blank" rel="noreferrer" className="text-brand underline">
      {children}
    </a>
  ),
  blockquote: ({ children }) => (
    <blockquote className="mb-2 border-l-2 border-border pl-3 italic text-muted-foreground">
      {children}
    </blockquote>
  ),
  hr: () => <hr className="my-3 border-border" />,
  pre: ({ children }) => (
    <pre className="mb-2 last:mb-0 overflow-x-auto rounded-md bg-muted p-3 text-xs text-foreground">
      {children}
    </pre>
  ),
  code: ({ className, children }) => {
    // Fenced (block) code carries a `language-*` class and is wrapped by <pre>;
    // leave its styling to <pre>. Everything else is inline code.
    const isBlock = /language-/.test(className || '');
    if (isBlock) {
      return <code className={className}>{children}</code>;
    }
    return (
      <code className="rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]">{children}</code>
    );
  },
  table: ({ children }) => (
    <div className="mb-2 overflow-x-auto">
      <table className="w-full border-collapse text-xs">{children}</table>
    </div>
  ),
  th: ({ children }) => (
    <th className="border border-border px-2 py-1 text-left font-semibold">{children}</th>
  ),
  td: ({ children }) => <td className="border border-border px-2 py-1">{children}</td>,
};

function MarkdownContent({ content }: { content: string }) {
  return (
    <div className="break-words">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={MARKDOWN_COMPONENTS}>
        {content}
      </ReactMarkdown>
    </div>
  );
}
