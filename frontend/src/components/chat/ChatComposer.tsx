"use client";

import { FormEvent, KeyboardEvent } from 'react';
import { Loader2, Send } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';

interface ChatComposerProps {
  value: string;
  onChange: (value: string) => void;
  onSend: () => void;
  placeholder: string;
  /** Disables typing entirely (no transcript yet, recording being saved, …). */
  disabled: boolean;
  /** Extra gate for the send button (e.g. no model picked). */
  sendDisabled?: boolean;
  isSending: boolean;
}

export function ChatComposer({
  value,
  onChange,
  onSend,
  placeholder,
  disabled,
  sendDisabled,
  isSending,
}: ChatComposerProps) {
  const handleSubmit = (e?: FormEvent) => {
    e?.preventDefault();
    if (!value.trim() || isSending || disabled || sendDisabled) return;
    onSend();
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSubmit();
    }
  };

  return (
    <form onSubmit={handleSubmit} className="border-t border-border bg-muted p-3">
      <div className="flex items-end gap-2">
        <Textarea
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder={placeholder}
          disabled={disabled || isSending}
          rows={2}
          className="flex-1 resize-none bg-background"
        />
        <Button
          type="submit"
          size="icon"
          disabled={!value.trim() || disabled || isSending || !!sendDisabled}
          aria-label="Send message"
        >
          {isSending ? <Loader2 className="h-4 w-4 animate-spin" /> : <Send className="h-4 w-4" />}
        </Button>
      </div>
      <p className="mt-1 text-[11px] text-muted-foreground/70">Enter to send · Shift+Enter for newline</p>
    </form>
  );
}
