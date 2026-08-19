'use client';

import React, { useCallback, useEffect, useState } from 'react';
import { useRouter } from 'next/navigation';
import { FolderKanban, FolderPlus, MoreVertical, Pencil, Plus, Trash2 } from 'lucide-react';
import { toast } from 'sonner';

import { Button } from '@/components/ui/button';
import { ConfirmationModal } from '@/components/ConfirmationModal/confirmation-modal';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { ProjectFormDialog } from '@/components/Projects/ProjectFormDialog';
import {
  Project,
  deleteProject,
  listProjects,
  notifyProjectsChanged,
  onProjectsChanged,
} from '@/lib/projectsApi';

function formatDate(value: string): string {
  const date = new Date(value);
  return isNaN(date.getTime())
    ? ''
    : date.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
}

export default function ProjectsPage() {
  const router = useRouter();
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [editing, setEditing] = useState<Project | null>(null);
  const [isFormOpen, setIsFormOpen] = useState(false);
  const [deleting, setDeleting] = useState<Project | null>(null);

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
    load();
  }, [load]);

  // Moves made elsewhere (a meeting's row menu, a project page) change the
  // counts shown here.
  useEffect(() => onProjectsChanged(load), [load]);

  const openCreate = () => {
    setEditing(null);
    setIsFormOpen(true);
  };

  const openEdit = (project: Project) => {
    setEditing(project);
    setIsFormOpen(true);
  };

  const confirmDelete = async () => {
    if (!deleting) return;
    const project = deleting;
    setDeleting(null);
    try {
      await deleteProject(project.id);
      setProjects((prev) => (prev ?? []).filter((p) => p.id !== project.id));
      notifyProjectsChanged();
      toast.success('Project deleted', {
        description: `Its ${project.meetingCount} ${
          project.meetingCount === 1 ? 'meeting is' : 'meetings are'
        } still in Meetings, just unfiled.`,
      });
    } catch (error) {
      console.error('Failed to delete project:', error);
      toast.error('Failed to delete project', {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  return (
    <div className="h-[calc(100vh-var(--titlebar-height))] bg-background flex flex-col">
      <div className="sticky top-0 z-10 bg-background/80 backdrop-blur border-b border-border">
        <div className="max-w-4xl mx-auto px-4 md:px-8 py-6 flex flex-col sm:flex-row sm:items-center gap-3">
          <div className="flex items-center gap-3">
            <FolderKanban className="w-6 h-6 text-muted-foreground" />
            <h1 className="text-3xl font-bold">Projects</h1>
            {projects && (
              <span className="text-sm text-muted-foreground ml-1">
                {projects.length} {projects.length === 1 ? 'project' : 'projects'}
              </span>
            )}
          </div>
          <Button onClick={openCreate} className="sm:ml-auto">
            <Plus className="w-4 h-4" />
            New project
          </Button>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto">
        <div className="max-w-4xl mx-auto p-4 md:p-8">
          {projects === null ? (
            <div className="text-muted-foreground">Loading…</div>
          ) : projects.length === 0 ? (
            <div className="flex flex-col items-center justify-center py-20 text-center text-muted-foreground">
              <FolderPlus className="w-10 h-10 mb-3 opacity-40" />
              <p className="text-lg">No projects yet</p>
              <p className="text-sm max-w-sm">
                Group related meetings into a project — a client, a team, a workstream.
              </p>
              <Button onClick={openCreate} className="mt-4">
                <Plus className="w-4 h-4" />
                New project
              </Button>
            </div>
          ) : (
            <ul className="grid gap-3 sm:grid-cols-2">
              {projects.map((project) => (
                <li key={project.id}>
                  <div className="group relative h-full rounded-lg border border-border bg-card hover:bg-accent transition-colors">
                    <button
                      onClick={() => router.push(`/project-details?id=${project.id}`)}
                      className="w-full h-full text-left p-4 pr-12"
                    >
                      <p className="font-medium truncate">{project.name}</p>
                      <p className="mt-1 text-sm text-muted-foreground line-clamp-2 min-h-[1.25rem]">
                        {project.description ?? ''}
                      </p>
                      <p className="mt-3 text-xs text-muted-foreground">
                        {project.meetingCount}{' '}
                        {project.meetingCount === 1 ? 'meeting' : 'meetings'} · created{' '}
                        {formatDate(project.createdAt)}
                      </p>
                    </button>
                    <div className="absolute right-2 top-3.5">
                      <DropdownMenu>
                        <DropdownMenuTrigger
                          onClick={(e) => e.stopPropagation()}
                          aria-label="Project actions"
                          className="flex-shrink-0 p-1 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent outline-none focus-visible:ring-2 focus-visible:ring-ring data-[state=open]:bg-accent data-[state=open]:text-foreground"
                        >
                          <MoreVertical className="w-4 h-4" />
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end" className="w-44">
                          <DropdownMenuItem onSelect={() => openEdit(project)}>
                            <Pencil className="w-4 h-4" />
                            Edit
                          </DropdownMenuItem>
                          <DropdownMenuItem
                            onSelect={() => setDeleting(project)}
                            className="text-destructive focus:text-destructive focus:bg-destructive/10"
                          >
                            <Trash2 className="w-4 h-4" />
                            Delete project
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </div>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>

      <ProjectFormDialog
        open={isFormOpen}
        onOpenChange={setIsFormOpen}
        project={editing}
        onSaved={load}
      />

      <ConfirmationModal
        isOpen={deleting !== null}
        title="Delete project"
        confirmLabel="Delete project"
        text={
          deleting
            ? `Delete "${deleting.name}"? Its ${deleting.meetingCount} ${
                deleting.meetingCount === 1 ? 'meeting' : 'meetings'
              } will stay in Meetings — only the project grouping goes away.`
            : ''
        }
        onConfirm={confirmDelete}
        onCancel={() => setDeleting(null)}
      />
    </div>
  );
}
