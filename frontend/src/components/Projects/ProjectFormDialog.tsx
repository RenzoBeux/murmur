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
  PROJECT_COLORS,
  PROJECT_COLOR_LABELS,
  ProjectColor,
  projectColorClasses,
  resolveProjectColor,
} from '@/lib/projectColors';
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

/** Create or rename a project — the same three fields either way. */
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
  const [color, setColor] = useState<ProjectColor>(PROJECT_COLORS[0]);
  const [isSaving, setIsSaving] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);

  // Reseed from the project (or the prefill) each time the dialog opens, so a
  // cancelled edit never leaks its draft into the next one. A new project gets
  // an arbitrary palette slot preselected so consecutive projects don't all
  // come out the same color; the user can pick another before saving.
  useEffect(() => {
    if (!open) return;
    setName(project?.name ?? initialName);
    setDescription(project?.description ?? '');
    setColor(
      project
        ? resolveProjectColor(project)
        : PROJECT_COLORS[Math.floor(Math.random() * PROJECT_COLORS.length)],
    );
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
        ? await updateProject(project!.id, trimmed, description, color)
        : await createProject(trimmed, description, color);
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
          <div>
            <p className="mb-2 text-xs font-medium text-muted-foreground">Color</p>
            <div className="flex flex-wrap gap-2" role="radiogroup" aria-label="Project color">
              {PROJECT_COLORS.map((option) => {
                const isSelected = option === color;
                return (
                  <button
                    key={option}
                    type="button"
                    role="radio"
                    aria-checked={isSelected}
                    aria-label={PROJECT_COLOR_LABELS[option]}
                    title={PROJECT_COLOR_LABELS[option]}
                    onClick={() => setColor(option)}
                    className={`h-7 w-7 rounded-full transition-transform outline-none focus-visible:ring-2 focus-visible:ring-ring ${
                      projectColorClasses(option).solid
                    } ${
                      isSelected
                        ? 'ring-2 ring-offset-2 ring-offset-background ring-foreground/60 scale-110'
                        : 'hover:scale-110'
                    }`}
                  />
                );
              })}
            </div>
          </div>
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
