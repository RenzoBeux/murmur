"use client";

import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';

/**
 * Markdown rendering shared by the chat bubbles and the project brief.
 *
 * Uses explicit utility classes rather than the `prose` (typography) plugin, for
 * tighter control over spacing — a chat bubble and a full-page document want
 * very different vertical rhythm, and that is exactly what `variant` selects.
 */
export type MarkdownVariant = 'chat' | 'document';

const CHAT_MARKDOWN_COMPONENTS: Parameters<typeof ReactMarkdown>[0]['components'] = {
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

// Only the block-scale rules differ: a standalone document needs real heading
// hierarchy and looser paragraph rhythm. Links, code, quotes and tables are
// already right, so they are inherited rather than restated.
const DOCUMENT_MARKDOWN_COMPONENTS: Parameters<typeof ReactMarkdown>[0]['components'] = {
  ...CHAT_MARKDOWN_COMPONENTS,
  p: ({ children }) => <p className="mb-3 leading-7">{children}</p>,
  ul: ({ children }) => <ul className="mb-4 list-disc space-y-1.5 pl-5">{children}</ul>,
  ol: ({ children }) => <ol className="mb-4 list-decimal space-y-1.5 pl-5">{children}</ol>,
  h1: ({ children }) => <h1 className="mb-3 mt-6 text-xl font-semibold">{children}</h1>,
  h2: ({ children }) => (
    <h2 className="mb-2 mt-6 text-lg font-semibold first:mt-0">{children}</h2>
  ),
  h3: ({ children }) => <h3 className="mb-1.5 mt-4 text-base font-semibold">{children}</h3>,
  table: ({ children }) => (
    <div className="mb-4 overflow-x-auto">
      <table className="w-full border-collapse text-sm">{children}</table>
    </div>
  ),
};

export function MarkdownContent({
  content,
  variant = 'chat',
}: {
  content: string;
  variant?: MarkdownVariant;
}) {
  return (
    <div className="break-words">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={
          variant === 'document' ? DOCUMENT_MARKDOWN_COMPONENTS : CHAT_MARKDOWN_COMPONENTS
        }
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}
