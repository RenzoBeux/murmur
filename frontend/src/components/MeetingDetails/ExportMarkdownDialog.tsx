"use client";

import { useState } from 'react';
import { FileText, Sparkles, Files } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '../ui/dialog';
import { Button } from '../ui/button';
import { ExportScope } from '@/hooks/meeting-details/useExportOperations';

interface ExportMarkdownDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  hasSummary: boolean;
  hasTranscripts: boolean;
  onExport: (scope: ExportScope) => Promise<void>;
}

interface ScopeOption {
  scope: ExportScope;
  label: string;
  description: string;
  icon: React.ComponentType<{ className?: string }>;
  needs: 'transcripts' | 'summary' | 'both';
}

const OPTIONS: ScopeOption[] = [
  {
    scope: 'both',
    label: 'Summary + Transcript',
    description: 'Full meeting export with the AI summary followed by the timestamped transcript.',
    icon: Files,
    needs: 'both',
  },
  {
    scope: 'summary',
    label: 'Summary only',
    description: 'Just the AI-generated summary in its current state.',
    icon: Sparkles,
    needs: 'summary',
  },
  {
    scope: 'transcript',
    label: 'Transcript only',
    description: 'Just the raw timestamped transcript.',
    icon: FileText,
    needs: 'transcripts',
  },
];

export function ExportMarkdownDialog({
  open,
  onOpenChange,
  hasSummary,
  hasTranscripts,
  onExport,
}: ExportMarkdownDialogProps) {
  const [busyScope, setBusyScope] = useState<ExportScope | null>(null);

  const isDisabled = (option: ScopeOption): boolean => {
    if (busyScope !== null) return true;
    if (option.needs === 'transcripts') return !hasTranscripts;
    if (option.needs === 'summary') return !hasSummary;
    return !hasSummary || !hasTranscripts;
  };

  const handleClick = async (scope: ExportScope) => {
    setBusyScope(scope);
    try {
      await onExport(scope);
      onOpenChange(false);
    } finally {
      setBusyScope(null);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(next) => { if (busyScope === null) onOpenChange(next); }}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Export to Markdown</DialogTitle>
          <DialogDescription>
            Choose what to include in the exported <code>.md</code> file. You will be prompted to pick a save location.
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-2 py-2">
          {OPTIONS.map((option) => {
            const Icon = option.icon;
            const disabled = isDisabled(option);
            const isBusy = busyScope === option.scope;
            return (
              <Button
                key={option.scope}
                variant="outline"
                disabled={disabled}
                onClick={() => handleClick(option.scope)}
                className="h-auto justify-start whitespace-normal text-left p-3"
              >
                <div className="flex items-start gap-3 w-full">
                  <Icon className="mt-0.5 h-5 w-5 flex-shrink-0" />
                  <div className="flex-1 min-w-0">
                    <div className="font-medium">
                      {option.label}
                      {isBusy && <span className="ml-2 text-xs text-muted-foreground">Exporting…</span>}
                    </div>
                    <div className="text-xs text-muted-foreground mt-1">
                      {disabled && !isBusy
                        ? option.needs === 'summary'
                          ? 'No summary generated yet.'
                          : option.needs === 'transcripts'
                          ? 'No transcripts available.'
                          : 'Requires both a summary and transcripts.'
                        : option.description}
                    </div>
                  </div>
                </div>
              </Button>
            );
          })}
        </div>
      </DialogContent>
    </Dialog>
  );
}
