'use client';

import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useRouter } from 'next/navigation';
import { Calendar, Clock, ListVideo } from 'lucide-react';
import { groupMeetingsByDate } from '@/lib/meetingGrouping';

interface MeetingRow {
  id: string;
  title: string;
  createdAt?: string;
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
  const [meetings, setMeetings] = useState<MeetingRow[] | null>(null);

  useEffect(() => {
    invoke<Array<{ id: string; title: string; created_at: string }>>('api_get_meetings')
      .then((rows) =>
        setMeetings(rows.map((r) => ({ id: r.id, title: r.title, createdAt: r.created_at }))),
      )
      .catch((e) => {
        console.error('Failed to load meetings:', e);
        setMeetings([]);
      });
  }, []);

  const groups = meetings ? groupMeetingsByDate(meetings) : [];

  return (
    <div className="h-[calc(100vh-var(--titlebar-height))] bg-background flex flex-col">
      <div className="sticky top-0 z-10 bg-background/80 backdrop-blur border-b border-border">
        <div className="max-w-4xl mx-auto px-4 md:px-8 py-6 flex items-center gap-3">
          <ListVideo className="w-6 h-6 text-muted-foreground" />
          <h1 className="text-3xl font-bold">Meetings</h1>
          {meetings && (
            <span className="text-sm text-muted-foreground ml-1">
              {meetings.length} {meetings.length === 1 ? 'meeting' : 'meetings'}
            </span>
          )}
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
                        <button
                          onClick={() => router.push(`/meeting-details?id=${m.id}`)}
                          className="w-full text-left rounded-lg border border-border bg-card hover:bg-accent transition-colors p-4 flex items-center gap-4"
                        >
                          <div className="flex-1 min-w-0">
                            <p className="font-medium truncate">{m.title}</p>
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
                        </button>
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
