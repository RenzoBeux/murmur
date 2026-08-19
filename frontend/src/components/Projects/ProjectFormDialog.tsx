'use client';

import React, { useEffect, useRef, useState } from 'react';
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
import { Textarea } from '@/components/ui/textarea';
import {
  Project,
  createProject,
  notifyProjectsChanged,
  updateProject,
} from '@/lib/projectsApi';

interface ProjectFormDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Set to edit an existing project; omit to create a new one. */
  project?: Project | null;
  /** Prefills the name field of a new project (e.g. from a "create" search box). */
  initialName?: string;
  onSaved?: (project: Project) => void;
}

/** Create or rename a project — the same two fields either way. */
export function ProjectFormDialog({
  open,
  onOpenChange,
  project = null,
  initialName = '',
  onSaved,
}: ProjectFormDialogProps) {
  const isEditing = project !== null;
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [isSaving, setIsSaving] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);

  // Reseed from the project (or the prefill) each time the dialog opens, so a
  // cancelled edit never leaks its draft into the next one.
  useEffect(() => {
    if (!open) return;
    setName(project?.name ?? initialName);
    setDescription(project?.description ?? '');
    const focus = () => {
      nameRef.current?.focus();
      nameRef.current?.select();
    };
    focus();
    const timer = window.setTimeout(focus, 0);
    return () => window.clearTimeout(timer);
  }, [open, project, initialName]);

  const save = async () => {
    const trimmed = name.trim();
    if (!trimmed) {
      toast.error('Project name cannot be empty');
      return;
    }

    setIsSaving(true);
    try {
      const saved = isEditing
        ? await updateProject(project!.id, trimmed, description)
        : await createProject(trimmed, description);
      notifyProjectsChanged();
      onSaved?.(saved);
      onOpenChange(false);
      toast.success(isEditing ? 'Project updated' : 'Project created');
    } catch (error) {
      console.error('Failed to save project:', error);
      toast.error(isEditing ? 'Failed to update project' : 'Failed to create project', {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[460px]" onOpenAutoFocus={(e) => e.preventDefault()}>
        <DialogHeader>
          <DialogTitle>{isEditing ? 'Edit project' : 'New project'}</DialogTitle>
        </DialogHeader>
        <div className="space-y-3">
          <Input
            ref={nameRef}
            aria-label="Project name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                save();
              }
            }}
            placeholder="Project name"
          />
          <Textarea
            aria-label="Project description"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="What is this project about? (optional)"
            rows={3}
          />
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)} disabled={isSaving}>
            Cancel
          </Button>
          <Button onClick={save} disabled={isSaving}>
            {isSaving ? 'Saving…' : isEditing ? 'Save' : 'Create project'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
