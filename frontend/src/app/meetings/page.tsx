'use client';

import React, { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useRouter } from 'next/navigation';
import { Calendar, Clock, FolderKanban, ListVideo, Search, X } from 'lucide-react';
import { MeetingActionsMenu } from '@/components/MeetingActions/MeetingActionsMenu';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { groupMeetingsByDate } from '@/lib/meetingGrouping';
import { listProjects, onProjectsChanged } from '@/lib/projectsApi';

interface MeetingRow {
  id: string;
  title: string;
  createdAt?: string;
  /** The project the meeting is filed under, or null when unfiled. */
  projectId?: string | null;
}

// Shape returned by the FTS5-backed `api_search_transcripts` command.
interface TranscriptSearchResult {
  id: string;
  title: string;
  matchContext: string;
  timestamp: string;
}

// A meeting that matched a search, with the reason it matched.
interface SearchHit extends MeetingRow {
  // The transcript snippet around the match, if the match came from content.
  matchContext?: string;
}

function parseDate(s?: string): Date | null {
  if (!s) return null;
  const d = new Date(s);
  return isNaN(d.getTime()) ? null : d;
}

function fmtDate(s?: string): string {
  const d = parseDate(s);
  return d ? d.toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric', year: 'numeric' }) : '';
}

function fmtTime(s?: string): string {
  const d = parseDate(s);
  return d ? d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' }) : '';
}

export default function MeetingsPage() {
  const router = useRouter();
  const { refetchMeetings } = useSidebar();
  const [meetings, setMeetings] = useState<MeetingRow[] | null>(null);
  const [projectNames, setProjectNames] = useState<Map<string, string>>(new Map());
  const [query, setQuery] = useState('');
  // Transcript-content matches from the backend FTS index (id -> snippet).
  const [contentMatches, setContentMatches] = useState<Map<string, string>>(new Map());
  const [isSearching, setIsSearching] = useState(false);

  const loadMeetings = useCallback(async () => {
    try {
      const rows = await invoke<
        Array<{ id: string; title: string; created_at: string; project_id: string | null }>
      >('api_get_meetings');
      setMeetings(
        rows.map((r) => ({
          id: r.id,
          title: r.title,
          createdAt: r.created_at,
          projectId: r.project_id,
        })),
      );
    } catch (e) {
      console.error('Failed to load meetings:', e);
      setMeetings([]);
    }
  }, []);

  // Project names for the card chips, keyed by id. Cheap enough to refetch
  // alongside the meetings whenever a move happens anywhere in the app.
  const loadProjects = useCallback(async () => {
    try {
      const projects = await listProjects();
      setProjectNames(new Map(projects.map((p) => [p.id, p.name])));
    } catch (e) {
      console.error('Failed to load projects:', e);
    }
  }, []);

  useEffect(() => {
    loadMeetings();
    loadProjects();
  }, [loadMeetings, loadProjects]);

  useEffect(
    () =>
      onProjectsChanged(() => {
        loadMeetings();
        loadProjects();
      }),
    [loadMeetings, loadProjects],
  );

  // Rename/trash mutate this page's list optimistically; the sidebar keeps its
  // own copy of the meetings, so it needs a refetch to stay in sync.
  const handleRenamed = useCallback(
    (meetingId: string, newTitle: string) => {
      setMeetings((prev) =>
        prev ? prev.map((m) => (m.id === meetingId ? { ...m, title: newTitle } : m)) : prev,
      );
      refetchMeetings();
    },
    [refetchMeetings],
  );

  const handleTrashed = useCallback(
    (meetingId: string) => {
      setMeetings((prev) => (prev ? prev.filter((m) => m.id !== meetingId) : prev));
      refetchMeetings();
    },
    [refetchMeetings],
  );

  const handleRestored = useCallback(async () => {
    await loadMeetings();
    await refetchMeetings();
  }, [loadMeetings, refetchMeetings]);

  // Debounced transcript search. A monotonically-increasing seq guards against
  // out-of-order responses clobbering the latest query's results.
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const seqRef = useRef(0);
  useEffect(() => {
    const q = query.trim();
    if (debounceRef.current) clearTimeout(debounceRef.current);

    if (!q) {
      seqRef.current += 1; // cancel any inflight search
      setContentMatches(new Map());
      setIsSearching(false);
      return;
    }

    setIsSearching(true);
    const mySeq = ++seqRef.current;
    debounceRef.current = setTimeout(() => {
      invoke<TranscriptSearchResult[]>('api_search_transcripts', { query: q })
        .then((results) => {
          if (mySeq !== seqRef.current) return; // stale response
          const map = new Map<string, string>();
          for (const r of results) if (!map.has(r.id)) map.set(r.id, r.matchContext);
          setContentMatches(map);
        })
        .catch((e) => {
          if (mySeq !== seqRef.current) return;
          console.error('Transcript search failed:', e);
          setContentMatches(new Map());
        })
        .finally(() => {
          if (mySeq === seqRef.current) setIsSearching(false);
        });
    }, 250);

    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, [query]);

  const q = query.trim().toLowerCase();
  const searching = q.length > 0;

  // When searching, a meeting is a hit if its title matches OR its transcript
  // content matched in the FTS index. Content matches carry a snippet.
  const hits: SearchHit[] | null =
    meetings === null
      ? null
      : searching
        ? meetings
            .filter((m) => m.title.toLowerCase().includes(q) || contentMatches.has(m.id))
            .map((m) => ({ ...m, matchContext: contentMatches.get(m.id) }))
        : meetings;

  const groups = !searching && hits ? groupMeetingsByDate(hits) : [];

  return (
    <div className="h-[calc(100vh-var(--titlebar-height))] bg-background flex flex-col">
      <div className="sticky top-0 z-10 bg-background/80 backdrop-blur border-b border-border">
        <div className="max-w-4xl mx-auto px-4 md:px-8 py-6 flex flex-col sm:flex-row sm:items-center gap-3">
          <div className="flex items-center gap-3">
            <ListVideo className="w-6 h-6 text-muted-foreground" />
            <h1 className="text-3xl font-bold">Meetings</h1>
            {meetings && (
              <span className="text-sm text-muted-foreground ml-1">
                {searching ? `${hits!.length} of ${meetings.length}` : meetings.length}
                {' '}
                {(searching ? hits!.length : meetings.length) === 1 ? 'meeting' : 'meetings'}
              </span>
            )}
          </div>
          <div className="relative sm:ml-auto w-full sm:w-72">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground pointer-events-none" />
            <input
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search titles & transcripts…"
              className="w-full rounded-lg border border-border bg-card pl-9 pr-9 py-2 text-sm outline-none focus:ring-2 focus:ring-brand/40"
            />
            {query && (
              <button
                onClick={() => setQuery('')}
                aria-label="Clear search"
                className="absolute right-2 top-1/2 -translate-y-1/2 p-1 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent"
              >
                <X className="w-4 h-4" />
              </button>
            )}
          </div>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto">
        <div className="max-w-4xl mx-auto p-4 md:p-8">
          {meetings === null ? (
            <div className="text-muted-foreground">Loading…</div>
          ) : meetings.length === 0 ? (
            <div className="flex flex-col items-center justify-center py-20 text-center text-muted-foreground">
              <ListVideo className="w-10 h-10 mb-3 opacity-40" />
              <p className="text-lg">No meetings yet</p>
              <p className="text-sm">Start a recording and it will show up here.</p>
            </div>
          ) : searching ? (
            // Search results: a flat list ranked by the backend, with the
            // transcript snippet shown when the match came from content.
            hits!.length === 0 ? (
              <div className="flex flex-col items-center justify-center py-20 text-center text-muted-foreground">
                <Search className="w-10 h-10 mb-3 opacity-40" />
                <p className="text-lg">{isSearching ? 'Searching…' : 'No matches'}</p>
                {!isSearching && <p className="text-sm">Nothing matches “{query}” in titles or transcripts.</p>}
              </div>
            ) : (
              <ul className="space-y-1.5">
                {hits!.map((m) => (
                  <li key={m.id}>
                    <MeetingCard
                      m={m}
                      projectName={m.projectId ? projectNames.get(m.projectId) : undefined}
                      onClick={() => router.push(`/meeting-details?id=${m.id}`)}
                      onRenamed={handleRenamed}
                      onTrashed={handleTrashed}
                      onRestored={handleRestored}
                    />
                  </li>
                ))}
              </ul>
            )
          ) : (
            <div className="space-y-6">
              {groups.map((group) => (
                <section key={group.key}>
                  <h2 className="sticky top-0 z-[1] bg-background/95 backdrop-blur py-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground/70">
                    {group.label}
                  </h2>
                  <ul className="mt-1 space-y-1.5">
                    {group.items.map((m) => (
                      <li key={m.id}>
                        <MeetingCard
                          m={m}
                          projectName={m.projectId ? projectNames.get(m.projectId) : undefined}
                          onClick={() => router.push(`/meeting-details?id=${m.id}`)}
                          onRenamed={handleRenamed}
                          onTrashed={handleTrashed}
                          onRestored={handleRestored}
                        />
                      </li>
                    ))}
                  </ul>
                </section>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function MeetingCard({
  m,
  projectName,
  onClick,
  onRenamed,
  onTrashed,
  onRestored,
}: {
  m: SearchHit;
  projectName?: string;
  onClick: () => void;
  onRenamed: (meetingId: string, newTitle: string) => void;
  onTrashed: (meetingId: string) => void;
  onRestored: () => void;
}) {
  return (
    // The card body is the button and the actions menu sits beside it — a
    // button nested inside a button would be invalid markup.
    <div className="group relative rounded-lg border border-border bg-card hover:bg-accent transition-colors">
      <button onClick={onClick} className="w-full text-left p-4 pr-12">
        <div className="flex items-center gap-4">
          <div className="flex-1 min-w-0">
            <p className="font-medium truncate">{m.title}</p>
            {projectName && (
              <span className="mt-1 inline-flex max-w-full items-center gap-1.5 rounded-full border border-border px-2 py-0.5 text-xs text-muted-foreground">
                <FolderKanban className="w-3 h-3 shrink-0" />
                <span className="truncate">{projectName}</span>
              </span>
            )}
          </div>
          <div className="shrink-0 flex items-center gap-4 text-xs text-muted-foreground">
            <span className="flex items-center gap-1.5">
              <Calendar className="w-3.5 h-3.5" />
              {fmtDate(m.createdAt)}
            </span>
            <span className="flex items-center gap-1.5">
              <Clock className="w-3.5 h-3.5" />
              {fmtTime(m.createdAt)}
            </span>
          </div>
        </div>
        {m.matchContext && (
          <div className="mt-2 text-xs text-muted-foreground bg-warning/10 border border-warning/20 rounded p-2 line-clamp-2">
            <span className="font-medium text-warning">Match:</span> {m.matchContext}
          </div>
        )}
      </button>
      <div className="absolute right-2 top-3.5">
        <MeetingActionsMenu
          meetingId={m.id}
          title={m.title}
          projectId={m.projectId ?? null}
          onRenamed={(newTitle) => onRenamed(m.id, newTitle)}
          onTrashed={() => onTrashed(m.id)}
          onRestored={onRestored}
        />
      </div>
    </div>
  );
}
