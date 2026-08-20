import { invoke } from '@tauri-apps/api/core';

/**
 * The stored cross-meeting brief for a project.
 *
 * Mirrors `ProjectSummaryResponse` in src-tauri/src/api/project_summary_api.rs.
 * Status comes straight off the database row, so a run started before the user
 * navigated away is still visible on return — the UI re-attaches with nothing
 * but the project id.
 */
export type ProjectSummaryStatus =
  | 'idle'
  | 'PENDING'
  | 'completed'
  | 'failed'
  | 'cancelled';

/** One meeting a stored brief was built from, snapshotted at generation time. */
export interface CoveredMeeting {
  id: string;
  title: string;
  createdAt: string;
  /** 'summary' | 'transcript' | 'none' — what it actually contributed. */
  source: string;
  fingerprint: string;
}

/** A meeting currently in the project, and what it can contribute. */
export interface ProjectMeetingContext {
  meetingId: string;
  title: string;
  hasSummary: boolean;
  hasTranscript: boolean;
}

export interface ProjectSummaryCoverage {
  covered: CoveredMeeting[];
  /** In the project now, absent from the brief. */
  added: ProjectMeetingContext[];
  /** In the brief, no longer in the project. */
  removed: CoveredMeeting[];
  /** Still in both, but its own summary was rewritten since. */
  changed: CoveredMeeting[];
  isStale: boolean;
}

export interface ProjectSummaryProgress {
  stage: string | null;
  current: number;
  total: number;
}

export interface ProjectSummary {
  status: ProjectSummaryStatus;
  markdown: string | null;
  error: string | null;
  generatedAt: string | null;
  provider: string | null;
  model: string | null;
  language: string | null;
  progress: ProjectSummaryProgress;
  coverage: ProjectSummaryCoverage;
  meetings: ProjectMeetingContext[];
}

export interface StartProjectSummaryResult {
  started: boolean;
  /** True when a run was already in flight — poll, don't treat it as an error. */
  alreadyRunning: boolean;
}

/** True while a brief is being generated. */
export function isGenerating(summary: ProjectSummary | null): boolean {
  return summary?.status === 'PENDING';
}

export async function getProjectSummary(projectId: string): Promise<ProjectSummary> {
  return invoke<ProjectSummary>('api_get_project_summary', { projectId });
}

export async function generateProjectSummary(
  projectId: string,
  provider: string,
  model: string,
  summaryLanguage?: string | null,
): Promise<StartProjectSummaryResult> {
  return invoke<StartProjectSummaryResult>('api_generate_project_summary', {
    projectId,
    provider,
    model,
    summaryLanguage: summaryLanguage ?? null,
  });
}

export async function cancelProjectSummary(projectId: string): Promise<void> {
  await invoke('api_cancel_project_summary', { projectId });
}

/** What the project chat can currently see, for the "context" notice. */
export async function getProjectChatContext(projectId: string): Promise<ProjectMeetingContext[]> {
  return invoke<ProjectMeetingContext[]>('api_project_chat_context', { projectId });
}

/**
 * Human wording for a generation stage. Kept beside the API rather than in the
 * panel so the vocabulary has one definition shared with the Rust side.
 */
export function stageLabel(progress: ProjectSummaryProgress): string {
  const { stage, current, total } = progress;
  switch (stage) {
    case 'collecting':
      return 'Reading the project’s meetings…';
    case 'reducing':
      return total > 0
        ? `Condensing the history (${current} of ${total})…`
        : 'Condensing the history…';
    case 'synthesizing':
      return 'Writing the brief…';
    default:
      return 'Generating project brief…';
  }
}

/** "2 meetings added since", "1 removed since", or both. */
export function stalenessLabel(coverage: ProjectSummaryCoverage): string | null {
  const parts: string[] = [];
  if (coverage.added.length > 0) parts.push(`${coverage.added.length} added`);
  if (coverage.removed.length > 0) parts.push(`${coverage.removed.length} removed`);
  if (coverage.changed.length > 0) parts.push(`${coverage.changed.length} updated`);
  if (parts.length === 0) return null;

  // "2 meetings added since" reads better than "2 added since" in the common
  // single-category case; the combined case stays terse.
  if (parts.length === 1 && coverage.added.length > 0) {
    const n = coverage.added.length;
    return `${n} ${n === 1 ? 'meeting' : 'meetings'} added since`;
  }
  return `${parts.join(', ')} since`;
}
