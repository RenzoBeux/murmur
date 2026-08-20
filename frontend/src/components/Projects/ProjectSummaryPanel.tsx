"use client";

import { ReactNode } from 'react';
import { useRouter } from 'next/navigation';
import {
  AlertTriangle,
  Copy,
  FileQuestion,
  FolderKanban,
  Loader2,
  Plus,
  RefreshCw,
  Sparkles,
  Square,
} from 'lucide-react';
import { toast } from 'sonner';

import { Button } from '@/components/ui/button';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { MarkdownContent } from '@/components/markdown/MarkdownContent';
import { ProjectSummary, stageLabel, stalenessLabel } from '@/lib/projectSummaryApi';

interface ProjectSummaryPanelProps {
  meetingCount: number;
  summary: ProjectSummary | null;
  isLoading: boolean;
  isGenerating: boolean;
  onGenerate: () => void;
  onCancel: () => void;
  onAddMeetings: () => void;
  /** The page owns model selection; the picker is rendered into the header. */
  modelPicker: ReactNode;
  hasModel: boolean;
}

function formatDateTime(value: string | null): string {
  if (!value) return '';
  const d = new Date(value);
  return isNaN(d.getTime())
    ? ''
    : d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
}

/**
 * The project Summary tab: one narrative read across every meeting.
 *
 * Read-only by design. The brief is derived and regenerable, so an editor would
 * promise durability it cannot keep — the next Regenerate would silently destroy
 * the edit. Copy-to-clipboard covers the real need without inheriting
 * `BlockNoteSummaryView`'s dirty/save contract, which every other feature would
 * then have to remember to flush.
 */
export function ProjectSummaryPanel({
  meetingCount,
  summary,
  isLoading,
  isGenerating,
  onGenerate,
  onCancel,
  onAddMeetings,
  modelPicker,
  hasModel,
}: ProjectSummaryPanelProps) {
  const markdown = summary?.markdown ?? null;
  const hasBrief = Boolean(markdown);

  if (isLoading) {
    // Skeleton rather than a centered spinner: a spinner that swaps for content
    // shifts the whole layout on every project open.
    return (
      <div className="mx-auto max-w-3xl space-y-3 px-6 py-8">
        <div className="h-5 w-1/3 animate-pulse rounded bg-muted" />
        <div className="h-4 w-full animate-pulse rounded bg-muted" />
        <div className="h-4 w-5/6 animate-pulse rounded bg-muted" />
      </div>
    );
  }

  if (meetingCount === 0) {
    return (
      <EmptyState
        icon={<FolderKanban className="h-10 w-10 opacity-40" />}
        title="Nothing to summarize yet"
        body="A project brief reads across the meetings in this project."
        action={
          <Button onClick={onAddMeetings}>
            <Plus className="h-4 w-4" />
            Add meetings
          </Button>
        }
      />
    );
  }

  if (!hasBrief && !isGenerating) {
    return (
      <EmptyState
        icon={<FileQuestion className="h-10 w-10 opacity-40" />}
        title="No project brief yet"
        body={`One narrative across all ${meetingCount} ${
          meetingCount === 1 ? 'meeting' : 'meetings'
        } — where things stand, recurring themes, decisions over time, open questions, and outstanding action items with the meeting each came from.`}
        action={
          <div className="flex flex-col items-center gap-2">
            <div className="flex items-center gap-2">
              {modelPicker}
              <Button onClick={onGenerate} disabled={!hasModel}>
                <Sparkles className="h-4 w-4" />
                Generate brief
              </Button>
            </div>
            {!hasModel && (
              <p className="text-xs text-muted-foreground">Pick a model to generate.</p>
            )}
          </div>
        }
      />
    );
  }

  return (
    <div className="flex h-full flex-col">
      {isGenerating ? (
        <GeneratingBar summary={summary} onCancel={onCancel} />
      ) : (
        <CoverageBar
          summary={summary}
          onGenerate={onGenerate}
          modelPicker={modelPicker}
          hasModel={hasModel}
        />
      )}

      {summary?.status === 'failed' && summary.error && (
        <div className="mx-4 mt-3 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {summary.error}
        </div>
      )}

      <div className="flex-1 min-h-0 overflow-y-auto">
        {markdown ? (
          // The previous brief stays readable while a new one generates — a
          // Regenerate that blanks the page loses the thing being replaced
          // before its replacement exists.
          <div
            className={`mx-auto max-w-3xl px-6 py-6 text-sm ${
              isGenerating ? 'pointer-events-none opacity-50' : ''
            }`}
          >
            <MarkdownContent content={markdown} variant="document" />
            {summary?.generatedAt && !isGenerating && (
              <p className="mt-8 border-t border-border pt-3 text-xs text-muted-foreground">
                Generated {summary.provider && summary.model
                  ? `with ${summary.provider}/${summary.model} `
                  : ''}
                on {formatDateTime(summary.generatedAt)}
              </p>
            )}
          </div>
        ) : (
          <div className="flex h-full items-center justify-center px-6 text-center text-sm text-muted-foreground">
            Reading this project’s meetings…
          </div>
        )}
      </div>
    </div>
  );
}

function GeneratingBar({
  summary,
  onCancel,
}: {
  summary: ProjectSummary | null;
  onCancel: () => void;
}) {
  const progress = summary?.progress ?? { stage: null, current: 0, total: 0 };
  const determinate = progress.total > 0;
  const pct = determinate ? Math.round((progress.current / progress.total) * 100) : 0;

  return (
    <div className="shrink-0 border-b border-border px-4 py-3">
      <div className="flex items-center gap-2 text-sm">
        <Loader2 className="h-4 w-4 shrink-0 animate-spin text-brand" />
        <span className="flex-1 truncate">{stageLabel(progress)}</span>
        <Button variant="ghost" size="sm" onClick={onCancel} className="gap-1.5">
          <Square className="h-3.5 w-3.5" />
          Stop
        </Button>
      </div>
      {determinate && (
        <div className="mt-2 h-1 w-full overflow-hidden rounded-full bg-muted">
          <div className="h-full bg-brand transition-all" style={{ width: `${pct}%` }} />
        </div>
      )}
      <p className="mt-1.5 text-[11px] text-muted-foreground">
        This reads every meeting in the project — it can take a few minutes.
      </p>
    </div>
  );
}

function CoverageBar({
  summary,
  onGenerate,
  modelPicker,
  hasModel,
}: {
  summary: ProjectSummary | null;
  onGenerate: () => void;
  modelPicker: ReactNode;
  hasModel: boolean;
}) {
  const router = useRouter();
  const coverage = summary?.coverage;
  const covered = (coverage?.covered ?? []).filter((c) => c.source !== 'none');
  const staleness = coverage ? stalenessLabel(coverage) : null;

  const copy = async () => {
    if (!summary?.markdown) return;
    try {
      await navigator.clipboard.writeText(summary.markdown);
      toast.success('Brief copied');
    } catch (err) {
      console.error('Failed to copy brief:', err);
      toast.error('Failed to copy brief');
    }
  };

  return (
    <div className="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-border px-4 py-2 text-xs text-muted-foreground">
      <Popover>
        <PopoverTrigger asChild>
          <button className="underline decoration-dotted underline-offset-2 hover:text-foreground">
            Covers {covered.length} {covered.length === 1 ? 'meeting' : 'meetings'}
          </button>
        </PopoverTrigger>
        <PopoverContent align="start" className="max-h-72 w-72 overflow-y-auto p-1">
          {covered.length === 0 ? (
            <p className="px-2 py-1.5 text-xs text-muted-foreground">No meetings covered.</p>
          ) : (
            covered.map((m) => (
              <button
                key={m.id}
                onClick={() => router.push(`/meeting-details?id=${m.id}`)}
                className="block w-full truncate rounded px-2 py-1.5 text-left text-xs hover:bg-accent"
              >
                {m.title}
              </button>
            ))
          )}
        </PopoverContent>
      </Popover>

      {summary?.generatedAt && (
        <>
          <span aria-hidden>·</span>
          <span>generated {formatDateTime(summary.generatedAt)}</span>
        </>
      )}

      <div className="ml-auto flex items-center gap-1.5">
        {staleness && (
          <span className="inline-flex items-center gap-1.5 rounded-full border border-amber-500/30 bg-amber-500/10 px-2 py-0.5 text-amber-600 dark:text-amber-500">
            <AlertTriangle className="h-3 w-3" />
            {staleness}
          </span>
        )}
        <Button variant="ghost" size="sm" onClick={copy} className="gap-1.5">
          <Copy className="h-3.5 w-3.5" />
          Copy
        </Button>
        {modelPicker}
        <Button variant="ghost" size="sm" onClick={onGenerate} disabled={!hasModel} className="gap-1.5">
          <RefreshCw className="h-3.5 w-3.5" />
          Regenerate
        </Button>
      </div>
    </div>
  );
}

function EmptyState({
  icon,
  title,
  body,
  action,
}: {
  icon: ReactNode;
  title: string;
  body: string;
  action: ReactNode;
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center px-6 py-16 text-center text-muted-foreground">
      {icon}
      <p className="mt-3 text-lg text-foreground">{title}</p>
      <p className="mt-2 max-w-md text-sm">{body}</p>
      <div className="mt-5">{action}</div>
    </div>
  );
}
