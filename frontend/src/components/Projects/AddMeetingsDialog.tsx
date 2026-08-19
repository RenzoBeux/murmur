'use client';

import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { Check, Search } from 'lucide-react';
import { toast } from 'sonner';

import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import {
  Project,
  ProjectMeeting,
  assignMeetingsToProject,
  listAllMeetings,
  listProjects,
  notifyProjectsChanged,
} from '@/lib/projectsApi';

interface AddMeetingsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  projectId: string;
  projectName: string;
  onAdded?: (count: number) => void;
}

function formatDate(value?: string): string {
  if (!value) return '';
  const date = new Date(value);
  return isNaN(date.getTime())
    ? ''
    : date.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
}

/** Multi-select picker for filing several meetings into one project at once. */
export function AddMeetingsDialog({
  open,
  onOpenChange,
  projectId,
  projectName,
  onAdded,
}: AddMeetingsDialogProps) {
  const [meetings, setMeetings] = useState<ProjectMeeting[] | null>(null);
  const [projectNames, setProjectNames] = useState<Map<string, string>>(new Map());
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState('');
  const [isSaving, setIsSaving] = useState(false);

  const load = useCallback(async () => {
    try {
      const [allMeetings, projects] = await Promise.all([listAllMeetings(), listProjects()]);
      // Meetings already in this project have nothing to add.
      setMeetings(allMeetings.filter((m) => m.projectId !== projectId));
      setProjectNames(new Map(projects.map((p: Project) => [p.id, p.name])));
    } catch (error) {
      console.error('Failed to load meetings:', error);
      toast.error('Failed to load meetings', {
        description: error instanceof Error ? error.message : String(error),
      });
      setMeetings([]);
    }
  }, [projectId]);

  useEffect(() => {
    if (!open) return;
    setQuery('');
    setSelected(new Set());
    load();
  }, [open, load]);

  const filtered = useMemo(() => {
    if (!meetings) return null;
    const q = query.trim().toLowerCase();
    return q ? meetings.filter((m) => m.title.toLowerCase().includes(q)) : meetings;
  }, [meetings, query]);

  const toggle = (id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const add = async () => {
    if (selected.size === 0) return;
    setIsSaving(true);
    try {
      const moved = await assignMeetingsToProject([...selected], projectId);
      notifyProjectsChanged();
      onAdded?.(moved);
      onOpenChange(false);
      toast.success(
        `${moved} ${moved === 1 ? 'meeting' : 'meetings'} added to "${projectName}"`,
      );
    } catch (error) {
      console.error('Failed to add meetings:', error);
      toast.error('Failed to add meetings', {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[560px]">
        <DialogHeader>
          <DialogTitle>Add meetings to “{projectName}”</DialogTitle>
        </DialogHeader>

        <div className="relative">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground pointer-events-none" />
          <Input
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search meetings…"
            aria-label="Search meetings"
            className="pl-9"
          />
        </div>

        <div className="max-h-80 overflow-y-auto -mx-1 px-1 space-y-1">
          {filtered === null ? (
            <p className="py-8 text-center text-sm text-muted-foreground">Loading…</p>
          ) : filtered.length === 0 ? (
            <p className="py-8 text-center text-sm text-muted-foreground">
              {query.trim()
                ? 'No meeting matches that search.'
                : 'Every meeting is already in this project.'}
            </p>
          ) : (
            filtered.map((meeting) => {
              const isSelected = selected.has(meeting.id);
              const otherProject = meeting.projectId
                ? projectNames.get(meeting.projectId)
                : undefined;
              return (
                <button
                  key={meeting.id}
                  onClick={() => toggle(meeting.id)}
                  aria-pressed={isSelected}
                  className={`w-full flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors ${
                    isSelected ? 'border-brand/40 bg-brand/10' : 'border-border hover:bg-accent'
                  }`}
                >
                  <span
                    className={`flex-shrink-0 flex items-center justify-center w-4 h-4 rounded border ${
                      isSelected ? 'bg-brand border-brand text-brand-foreground' : 'border-border'
                    }`}
                  >
                    {isSelected && <Check className="w-3 h-3" />}
                  </span>
                  <span className="flex-1 min-w-0">
                    <span className="block text-sm font-medium truncate">{meeting.title}</span>
                    <span className="block text-xs text-muted-foreground">
                      {formatDate(meeting.createdAt)}
                      {otherProject && ` · moves out of "${otherProject}"`}
                    </span>
                  </span>
                </button>
              );
            })
          )}
        </div>

        <DialogFooter className="sm:justify-between">
          <span className="text-sm text-muted-foreground self-center">
            {selected.size} selected
          </span>
          <div className="flex gap-2">
            <Button variant="ghost" onClick={() => onOpenChange(false)} disabled={isSaving}>
              Cancel
            </Button>
            <Button onClick={add} disabled={selected.size === 0 || isSaving}>
              {isSaving ? 'Adding…' : 'Add to project'}
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
