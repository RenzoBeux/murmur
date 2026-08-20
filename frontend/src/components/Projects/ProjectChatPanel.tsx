"use client";

import { useMemo } from 'react';
import { AlertTriangle, FileText } from 'lucide-react';
import { ChatPanelShell } from '@/components/chat/ChatPanelShell';
import { ChatScope } from '@/lib/chatScope';
import { ProjectMeetingContext } from '@/lib/projectSummaryApi';

interface ProjectChatPanelProps {
  projectId: string;
  meetingCount: number;
  /** Per-meeting availability. null while loading. */
  context: ProjectMeetingContext[] | null;
}

const PROJECT_SUGGESTIONS = [
  'What are the open action items across these meetings?',
  'What decisions have we made so far?',
  'What has changed since the first meeting?',
  'What questions are still unanswered?',
];

/**
 * The project Chat tab. Everything mechanical lives in `ChatPanelShell`, which
 * the meeting chat shares; this supplies project-specific copy and the notice
 * saying what the assistant can actually see.
 */
export function ProjectChatPanel({ projectId, meetingCount, context }: ProjectChatPanelProps) {
  const scope = useMemo<ChatScope>(() => ({ kind: 'project', projectId }), [projectId]);

  // Gated on meetings, not summaries: a project of un-summarized meetings still
  // answers from transcripts, just more weakly. That is a notice, not a block.
  const canChat = meetingCount > 0;

  return (
    <ChatPanelShell
      scope={scope}
      canChat={canChat}
      groundingScope="project"
      placeholder={
        canChat
          ? `Ask across all ${meetingCount} ${meetingCount === 1 ? 'meeting' : 'meetings'}…`
          : 'Add meetings to this project first.'
      }
      emptyState={{
        body: canChat
          ? 'Ask across every meeting in this project. The assistant reads each meeting’s summary, and the full transcripts of as many meetings as fit.'
          : 'Add meetings to this project, then ask questions that span all of them.',
        suggestions: PROJECT_SUGGESTIONS,
      }}
      contextNotice={canChat ? <ProjectChatContextNotice context={context} /> : null}
    />
  );
}

/**
 * What the assistant can see, stated up front.
 *
 * The chat cannot always hold every transcript, and an answer built from
 * summaries alone reads exactly like one built from everything. Saying which is
 * the difference between partial coverage and *silent* partial coverage.
 */
function ProjectChatContextNotice({ context }: { context: ProjectMeetingContext[] | null }) {
  if (!context || context.length === 0) return null;

  const total = context.length;
  const summarized = context.filter((m) => m.hasSummary).length;

  if (summarized === 0) {
    return (
      <div className="flex items-center gap-1.5 border-b border-border bg-amber-500/10 px-4 py-1.5 text-[11px] text-amber-600 dark:text-amber-500">
        <AlertTriangle className="h-3 w-3 shrink-0" />
        No meeting summaries yet — answers lean on raw transcripts and will be thinner. Generate
        summaries on the meetings first.
      </div>
    );
  }

  return (
    <div className="flex items-center gap-1.5 border-b border-border bg-muted/40 px-4 py-1.5 text-[11px] text-muted-foreground">
      <FileText className="h-3 w-3 shrink-0" />
      {summarized === total ? (
        <span>
          Sees summaries of all {total} {total === 1 ? 'meeting' : 'meetings'}, plus the full
          transcripts of as many as fit.
        </span>
      ) : (
        <span>
          Sees {summarized} of {total} meeting summaries · {total - summarized} have no summary yet
        </span>
      )}
    </div>
  );
}
