'use client';

import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { Check, FolderPlus, Search, Slash } from 'lucide-react';
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
import { ProjectFormDialog } from '@/components/Projects/ProjectFormDialog';
import {
  Project,
  assignMeetingsToProject,
  getMeetingProject,
  listProjects,
  notifyProjectsChanged,
} from '@/lib/projectsApi';

interface MoveToProjectDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The meetings to file. One from a row menu, many from a bulk selection. */
  meetingIds: string[];
  /**
   * Current project of the meeting(s), so the dialog can mark it as active.
   * Leave undefined and the dialog looks it up itself for a single meeting.
   */
  currentProjectId?: string | null;
  /** Called after the move lands, with the destination (null = unfiled). */
  onMoved?: (project: Project | null) => void;
}

/** Pick the project a meeting (or a batch of them) should live in. */
export function MoveToProjectDialog({
  open,
  onOpenChange,
  meetingIds,
  currentProjectId,
  onMoved,
}: MoveToProjectDialogProps) {
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [query, setQuery] = useState('');
  const [busyId, setBusyId] = useState<string | null>(null);
  const [isCreating, setIsCreating] = useState(false);
  // Where the meeting lives now. Callers that already know it pass it in;
  // callers that don't (the sidebar rows) get it looked up on open.
  const [activeProjectId, setActiveProjectId] = useState<string | null>(currentProjectId ?? null);

  const load = useCallback(async () => {
    try {
      setProjects(await listProjects());
    } catch (error) {
      console.error('Failed to load projects:', error);
      toast.error('Failed to load projects', {
        description: error instanceof Error ? error.message : String(error),
      });
      setProjects([]);
    }
  }, []);

  useEffect(() => {
    if (!open) return;
    setQuery('');
    load();
  }, [open, load]);

  // meetingIds is usually a fresh array literal per render, so key the effect on
  // its contents rather than its identity.
  const meetingKey = meetingIds.join(',');
  useEffect(() => {
    if (!open) return;
    if (currentProjectId !== undefined) {
      setActiveProjectId(currentProjectId);
      return;
    }
    const ids = meetingKey ? meetingKey.split(',') : [];
    if (ids.length !== 1) {
      setActiveProjectId(null);
      return;
    }
    let cancelled = false;
    getMeetingProject(ids[0])
      .then((project) => {
        if (!cancelled) setActiveProjectId(project?.id ?? null);
      })
      .catch((error) => console.error('Failed to resolve meeting project:', error));
    return () => {
      cancelled = true;
    };
  }, [open, currentProjectId, meetingKey]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!projects) return null;
    return q ? projects.filter((p) => p.name.toLowerCase().includes(q)) : projects;
  }, [projects, query]);

  const move = async (project: Project | null) => {
    // "None" on an already-unfiled meeting, or the project it is already in.
    if ((project?.id ?? null) === activeProjectId) {
      onOpenChange(false);
      return;
    }

    setBusyId(project?.id ?? 'none');
    try {
      await assignMeetingsToProject(meetingIds, project?.id ?? null);
      notifyProjectsChanged();
      onMoved?.(project);
      onOpenChange(false);
      const what = meetingIds.length === 1 ? 'Meeting' : `${meetingIds.length} meetings`;
      toast.success(
        project ? `${what} moved to "${project.name}"` : `${what} removed from project`,
      );
    } catch (error) {
      console.error('Failed to move meetings:', error);
      toast.error('Failed to move meetings', {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setBusyId(null);
    }
  };

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="sm:max-w-[460px]">
          <DialogHeader>
            <DialogTitle>
              {meetingIds.length === 1
                ? 'Move to project'
                : `Move ${meetingIds.length} meetings to project`}
            </DialogTitle>
          </DialogHeader>

          <div className="relative">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground pointer-events-none" />
            <Input
              autoFocus
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Find a project…"
              aria-label="Find a project"
              className="pl-9"
            />
          </div>

          <div className="max-h-72 overflow-y-auto -mx-1 px-1 space-y-1">
            {filtered === null ? (
              <p className="py-6 text-center text-sm text-muted-foreground">Loading…</p>
            ) : (
              <>
                {filtered.map((project) => {
                  const isCurrent = project.id === activeProjectId;
                  return (
                    <button
                      key={project.id}
                      onClick={() => move(project)}
                      disabled={busyId !== null}
                      className={`w-full flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors disabled:opacity-60 ${
                        isCurrent
                          ? 'border-brand/40 bg-brand/10'
                          : 'border-border hover:bg-accent'
                      }`}
                    >
                      <div className="flex-1 min-w-0">
                        <p className="text-sm font-medium truncate">{project.name}</p>
                        <p className="text-xs text-muted-foreground">
                          {project.meetingCount}{' '}
                          {project.meetingCount === 1 ? 'meeting' : 'meetings'}
                        </p>
                      </div>
                      {isCurrent && <Check className="w-4 h-4 text-brand shrink-0" />}
                    </button>
                  );
                })}

                {filtered.length === 0 && (
                  <p className="py-6 text-center text-sm text-muted-foreground">
                    {query.trim() ? 'No project matches that name.' : 'No projects yet.'}
                  </p>
                )}

                {activeProjectId && (
                  <button
                    onClick={() => move(null)}
                    disabled={busyId !== null}
                    className="w-full flex items-center gap-2 rounded-lg border border-border px-3 py-2.5 text-left text-sm text-muted-foreground hover:bg-accent hover:text-foreground transition-colors disabled:opacity-60"
                  >
                    <Slash className="w-4 h-4 shrink-0" />
                    Remove from project
                  </button>
                )}
              </>
            )}
          </div>

          <DialogFooter className="sm:justify-between">
            <Button variant="ghost" onClick={() => setIsCreating(true)}>
              <FolderPlus className="w-4 h-4" />
              New project
            </Button>
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Creating from here files the meetings into the new project right away —
          that is the only reason someone opens it mid-move. */}
      <ProjectFormDialog
        open={isCreating}
        onOpenChange={setIsCreating}
        initialName={query.trim()}
        onSaved={(project) => move(project)}
      />
    </>
  );
}
