'use client';

import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { Check, FileText, Loader2, Mic, Paperclip, ScrollText } from 'lucide-react';
import { toast } from 'sonner';

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import {
  DEFAULT_EXPORT_CONTENTS,
  DEFAULT_TRANSCRIPT_FORMAT,
  ExportAvailability,
  ExportBundleScope,
  ExportContents,
  MeetingExportInfo,
  TranscriptFormat,
  exportBundle,
  fetchExportAvailability,
  formatExportBytes,
} from '@/lib/exportBundleApi';

export type ExportBundleTarget =
  | { kind: 'meeting'; meetingId: string; title: string }
  | { kind: 'project'; projectId: string; name: string };

interface ExportBundleDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  target: ExportBundleTarget;
  /**
   * Meeting scope only: flush unsaved summary edits before Rust reads the DB.
   * Without this, edits still sitting in the BlockNote editor would silently
   * miss the archive.
   */
  onBeforeExport?: () => Promise<void>;
}

type ContentKey = keyof ExportContents;

function formatDate(value: string): string {
  const date = new Date(value);
  return isNaN(date.getTime())
    ? ''
    : date.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
}

/** Roll per-meeting availability up into the totals the include rows show. */
function summarize(meetings: MeetingExportInfo[]) {
  return {
    withTranscript: meetings.filter((m) => m.transcriptSegments > 0).length,
    segments: meetings.reduce((sum, m) => sum + m.transcriptSegments, 0),
    withSummary: meetings.filter((m) => m.hasSummary).length,
    attachmentCount: meetings.reduce((sum, m) => sum + m.attachmentCount, 0),
    attachmentBytes: meetings.reduce((sum, m) => sum + m.attachmentBytes, 0),
    withAudio: meetings.filter((m) => m.audioBytes !== null).length,
    audioBytes: meetings.reduce((sum, m) => sum + (m.audioBytes ?? 0), 0),
  };
}

/**
 * "What goes in the zip" picker, shared by the per-meeting and per-project
 * entry points. Anything the selection doesn't have is shown disabled with the
 * reason inline, so the user never picks something that would produce nothing.
 */
export function ExportBundleDialog({
  open,
  onOpenChange,
  target,
  onBeforeExport,
}: ExportBundleDialogProps) {
  const isProject = target.kind === 'project';

  const [availability, setAvailability] = useState<ExportAvailability | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [contents, setContents] = useState<ExportContents>(DEFAULT_EXPORT_CONTENTS);
  const [format, setFormat] = useState<TranscriptFormat>(DEFAULT_TRANSCRIPT_FORMAT);
  const [selectedMeetings, setSelectedMeetings] = useState<Set<string>>(new Set());
  const [isExporting, setIsExporting] = useState(false);

  const scopeKey = target.kind === 'meeting' ? target.meetingId : target.projectId;

  // Probe on the closed→open transition, never on mount: `useMeetingActions`
  // keeps its dialogs mounted after first use, so a mount-time effect would
  // fire for every list row that ever opened any of its dialogs.
  useEffect(() => {
    if (!open) return;

    let cancelled = false;
    setAvailability(null);
    setLoadError(null);
    setContents(DEFAULT_EXPORT_CONTENTS);
    setFormat(DEFAULT_TRANSCRIPT_FORMAT);
    setSelectedMeetings(new Set());

    const scope: ExportBundleScope =
      target.kind === 'meeting'
        ? { kind: 'meeting', meetingId: target.meetingId }
        : { kind: 'project', projectId: target.projectId, meetingIds: [] };

    fetchExportAvailability(scope)
      .then((result) => {
        if (cancelled) return;
        setAvailability(result);
        // Every meeting starts checked; the list is there to remove a few.
        setSelectedMeetings(new Set(result.meetings.map((m) => m.meetingId)));
      })
      .catch((error) => {
        if (cancelled) return;
        console.error('Failed to load export availability:', error);
        setLoadError(error instanceof Error ? error.message : String(error));
      });

    return () => {
      cancelled = true;
    };
    // `scopeKey` stands in for the target identity; the object is rebuilt each render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, scopeKey, target.kind]);

  const chosenMeetings = useMemo(() => {
    if (!availability) return [];
    if (!isProject) return availability.meetings;
    return availability.meetings.filter((m) => selectedMeetings.has(m.meetingId));
  }, [availability, isProject, selectedMeetings]);

  const stats = useMemo(() => summarize(chosenMeetings), [chosenMeetings]);

  // Deliberately over the FULL meeting set, not the selection: emptying the
  // project checklist must not swap the dialog for the empty state, or the
  // user loses the list they need to re-check something.
  const overallStats = useMemo(
    () => summarize(availability?.meetings ?? []),
    [availability],
  );

  const rows: Array<{
    key: ContentKey;
    label: string;
    icon: React.ReactNode;
    hint: string;
    available: boolean;
    unavailableHint: string;
    target: string;
  }> = [
    {
      key: 'transcript',
      label: isProject ? 'Transcripts' : 'Transcript',
      icon: <ScrollText className="w-4 h-4" />,
      hint: isProject
        ? `${stats.withTranscript} of ${chosenMeetings.length} meetings`
        : `${stats.segments} segments`,
      available: stats.withTranscript > 0,
      unavailableHint: isProject ? 'No transcripts yet' : 'No transcript yet',
      target: 'transcript.md',
    },
    {
      key: 'summary',
      label: isProject ? 'Summaries' : 'Summary',
      icon: <FileText className="w-4 h-4" />,
      hint: isProject
        ? `${stats.withSummary} of ${chosenMeetings.length} meetings`
        : 'Generated summary',
      available: stats.withSummary > 0,
      unavailableHint: 'No summary generated yet',
      target: 'summary.md',
    },
    {
      key: 'attachments',
      label: 'Attached files',
      icon: <Paperclip className="w-4 h-4" />,
      hint: `${stats.attachmentCount} ${stats.attachmentCount === 1 ? 'file' : 'files'} · ${formatExportBytes(stats.attachmentBytes)}`,
      available: stats.attachmentCount > 0,
      unavailableHint: 'No files attached',
      target: 'files/',
    },
    {
      key: 'audio',
      label: isProject ? 'Audio recordings' : 'Audio recording',
      icon: <Mic className="w-4 h-4" />,
      hint: isProject
        ? `${stats.withAudio} ${stats.withAudio === 1 ? 'recording' : 'recordings'} · ${formatExportBytes(stats.audioBytes)}`
        : formatExportBytes(stats.audioBytes),
      available: stats.withAudio > 0,
      unavailableHint: 'No recording on disk',
      target: 'recording',
    },
  ];

  const chosenCount = rows.filter((row) => row.available && contents[row.key]).length;
  const nothingAvailable =
    availability !== null &&
    overallStats.withTranscript === 0 &&
    overallStats.withSummary === 0 &&
    overallStats.attachmentCount === 0 &&
    overallStats.withAudio === 0;
  const estimatedBytes =
    (contents.attachments ? stats.attachmentBytes : 0) + (contents.audio ? stats.audioBytes : 0);

  const toggleContent = (key: ContentKey) =>
    setContents((prev) => ({ ...prev, [key]: !prev[key] }));

  const toggleMeeting = (id: string) =>
    setSelectedMeetings((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const handleExport = useCallback(async () => {
    if (!availability) return;
    setIsExporting(true);
    try {
      // Persist in-flight summary edits so the archive matches what's on screen.
      await onBeforeExport?.();

      const scope: ExportBundleScope =
        target.kind === 'meeting'
          ? { kind: 'meeting', meetingId: target.meetingId }
          : {
              kind: 'project',
              projectId: target.projectId,
              meetingIds: [...selectedMeetings],
            };

      const result = await exportBundle(scope, contents, format);

      // The native picker renders on top of this dialog, so a cancel there
      // should leave the selections exactly where the user left them.
      if (result.path === null) return;

      onOpenChange(false);

      const headline =
        target.kind === 'project'
          ? `Exported ${result.meetingsExported} ${result.meetingsExported === 1 ? 'meeting' : 'meetings'}`
          : 'Meeting exported';
      const description =
        result.warnings.length > 0
          ? `${result.path}\n${result.warnings.length} item${result.warnings.length === 1 ? '' : 's'} skipped`
          : result.path;

      toast.success(headline, { description });
      if (result.warnings.length > 0) {
        console.warn('Export warnings:', result.warnings);
      }
    } catch (error) {
      console.error('Export failed:', error);
      toast.error('Export failed', {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setIsExporting(false);
    }
  }, [availability, contents, format, onBeforeExport, onOpenChange, selectedMeetings, target]);

  const title = target.kind === 'meeting' ? target.title : target.name;
  const canExport =
    availability !== null &&
    chosenCount > 0 &&
    !isExporting &&
    (!isProject || selectedMeetings.size > 0);

  return (
    <Dialog
      open={open}
      // Never let a click-away or Esc drop a running export.
      onOpenChange={(next) => {
        if (!isExporting) onOpenChange(next);
      }}
    >
      <DialogContent className="sm:max-w-[560px]">
        <DialogHeader>
          <DialogTitle className="truncate">Export “{title}”</DialogTitle>
          <DialogDescription>
            {isProject
              ? 'Each meeting gets its own folder. Anything a meeting doesn’t have is skipped.'
              : 'Pick what goes into the .zip. You’ll choose where to save it next.'}
          </DialogDescription>
        </DialogHeader>

        {loadError ? (
          <p className="py-8 text-center text-sm text-destructive">
            Couldn’t read this {target.kind}: {loadError}
          </p>
        ) : availability === null ? (
          <p className="py-8 text-center text-sm text-muted-foreground">Checking what’s available…</p>
        ) : nothingAvailable ? (
          <p className="py-8 text-center text-sm text-muted-foreground">
            Nothing to export yet — record a meeting or generate a summary first.
          </p>
        ) : (
          <div className="space-y-3">
            <div className="space-y-1">
              {rows.map((row) => {
                const isOn = row.available && contents[row.key];
                return (
                  <button
                    key={row.key}
                    type="button"
                    onClick={() => toggleContent(row.key)}
                    aria-pressed={isOn}
                    disabled={!row.available || isExporting}
                    className={`w-full flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors disabled:opacity-50 disabled:cursor-not-allowed ${
                      isOn ? 'border-brand/40 bg-brand/10' : 'border-border hover:bg-accent'
                    }`}
                  >
                    <span
                      className={`flex-shrink-0 flex items-center justify-center w-4 h-4 rounded border ${
                        isOn ? 'bg-brand border-brand text-brand-foreground' : 'border-border'
                      }`}
                    >
                      {isOn && <Check className="w-3 h-3" />}
                    </span>
                    <span className="flex-shrink-0 text-muted-foreground">{row.icon}</span>
                    <span className="flex-1 min-w-0">
                      <span className="block text-sm font-medium">{row.label}</span>
                      <span className="block text-xs text-muted-foreground">
                        {row.available ? row.hint : row.unavailableHint}
                      </span>
                    </span>
                    <code className="flex-shrink-0 text-xs text-muted-foreground/70">
                      {row.target}
                    </code>
                  </button>
                );
              })}
            </div>

            {contents.transcript && stats.withTranscript > 0 && (
              <div className="rounded-lg border border-border px-3 py-1.5">
                <label className="flex items-center justify-between py-1.5 text-sm cursor-pointer">
                  <span>Include timestamps</span>
                  <Switch
                    checked={format.includeTimestamps}
                    disabled={isExporting}
                    onCheckedChange={(value) =>
                      setFormat((prev) => ({ ...prev, includeTimestamps: value }))
                    }
                  />
                </label>
                <label className="flex items-center justify-between py-1.5 text-sm cursor-pointer">
                  <span>Include speaker labels</span>
                  <Switch
                    checked={format.includeSpeakers}
                    disabled={isExporting}
                    onCheckedChange={(value) =>
                      setFormat((prev) => ({ ...prev, includeSpeakers: value }))
                    }
                  />
                </label>
              </div>
            )}

            {isProject && (
              <div>
                <p className="mb-1 text-xs font-medium text-muted-foreground">
                  Meetings ({selectedMeetings.size} of {availability.meetings.length})
                </p>
                <div className="max-h-56 overflow-y-auto -mx-1 px-1 space-y-1">
                  {availability.meetings.map((meeting) => {
                    const isSelected = selectedMeetings.has(meeting.meetingId);
                    return (
                      <button
                        key={meeting.meetingId}
                        type="button"
                        onClick={() => toggleMeeting(meeting.meetingId)}
                        aria-pressed={isSelected}
                        disabled={isExporting}
                        className={`w-full flex items-center gap-3 rounded-lg border px-3 py-2 text-left transition-colors disabled:opacity-50 ${
                          isSelected
                            ? 'border-brand/40 bg-brand/10'
                            : 'border-border hover:bg-accent'
                        }`}
                      >
                        <span
                          className={`flex-shrink-0 flex items-center justify-center w-4 h-4 rounded border ${
                            isSelected
                              ? 'bg-brand border-brand text-brand-foreground'
                              : 'border-border'
                          }`}
                        >
                          {isSelected && <Check className="w-3 h-3" />}
                        </span>
                        <span className="flex-1 min-w-0">
                          <span className="block text-sm truncate">{meeting.title}</span>
                        </span>
                        <span className="flex-shrink-0 text-xs text-muted-foreground">
                          {formatDate(meeting.createdAt)}
                        </span>
                      </button>
                    );
                  })}
                </div>
              </div>
            )}
          </div>
        )}

        <DialogFooter className="sm:justify-between">
          <span className="text-sm text-muted-foreground self-center">
            {availability === null || nothingAvailable
              ? ''
              : estimatedBytes > 0
                ? `${chosenCount} included · ~${formatExportBytes(estimatedBytes)}`
                : `${chosenCount} included`}
          </span>
          <div className="flex gap-2">
            <Button variant="ghost" onClick={() => onOpenChange(false)} disabled={isExporting}>
              {nothingAvailable ? 'Close' : 'Cancel'}
            </Button>
            {!nothingAvailable && (
              <Button
                onClick={handleExport}
                disabled={!canExport}
                title={chosenCount === 0 ? 'Pick at least one thing to include' : undefined}
              >
                {isExporting ? (
                  <>
                    <Loader2 className="w-4 h-4 animate-spin" />
                    Exporting…
                  </>
                ) : (
                  'Export .zip'
                )}
              </Button>
            )}
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
