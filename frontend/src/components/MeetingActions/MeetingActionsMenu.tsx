'use client';

import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { FolderInput, MoreVertical, Pencil, Trash2 } from 'lucide-react';
import { toast } from 'sonner';

import { ConfirmationModal } from '@/components/ConfirmationModal/confirmation-modal';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { MoveToProjectDialog } from '@/components/Projects/MoveToProjectDialog';
import type { Project } from '@/lib/projectsApi';

export interface MeetingActionsOptions {
  meetingId: string;
  /** Current title, used to seed the rename field and the toasts. */
  title: string;
  /** Called with the new title once it has been persisted. */
  onRenamed?: (newTitle: string) => void;
  /** Called once the meeting has been moved to the trash. */
  onTrashed?: () => void;
  /** Called after an Undo restored the meeting from the trash. */
  onRestored?: () => void;
  /** The project the meeting is currently filed under, if the caller knows it. */
  projectId?: string | null;
  /** Called once the meeting has been filed (null = removed from its project). */
  onMovedToProject?: (project: Project | null) => void;
}

/**
 * Rename / move-to-trash for a single meeting: the Tauri calls, the toasts and
 * the two dialogs in one place, so the sidebar, the meetings list and the
 * meeting-details header all behave identically.
 *
 * Render the returned `dialogs` node once inside the consumer tree.
 */
export function useMeetingActions({
  meetingId,
  title,
  onRenamed,
  onTrashed,
  onRestored,
  projectId,
  onMovedToProject,
}: MeetingActionsOptions) {
  const [isRenaming, setIsRenaming] = useState(false);
  const [draftTitle, setDraftTitle] = useState(title);
  const [isConfirmingTrash, setIsConfirmingTrash] = useState(false);
  const [isMovingToProject, setIsMovingToProject] = useState(false);
  const titleInputRef = React.useRef<HTMLInputElement>(null);

  // Focus the field ourselves once the dialog is open. Radix's own autofocus is
  // suppressed below (onOpenAutoFocus) because it loses to the focus the
  // trigger — a menu item or the header title button — is still settling. The
  // second pass runs once the closing menu has released its focus trap.
  useEffect(() => {
    if (!isRenaming) return;

    const focusField = () => {
      titleInputRef.current?.focus();
      titleInputRef.current?.select();
    };

    focusField();
    const timer = window.setTimeout(focusField, 0);
    return () => window.clearTimeout(timer);
  }, [isRenaming]);

  const openRename = useCallback(() => {
    setDraftTitle(title);
    setIsRenaming(true);
  }, [title]);

  const openTrash = useCallback(() => setIsConfirmingTrash(true), []);

  const openMoveToProject = useCallback(() => setIsMovingToProject(true), []);

  const closeRename = useCallback(() => {
    setIsRenaming(false);
    setDraftTitle(title);
  }, [title]);

  const confirmRename = useCallback(async () => {
    const newTitle = draftTitle.trim();

    if (!newTitle) {
      toast.error('Meeting title cannot be empty');
      return;
    }

    if (newTitle === title) {
      setIsRenaming(false);
      return;
    }

    try {
      await invoke('api_save_meeting_title', { meetingId, title: newTitle });
      setIsRenaming(false);
      onRenamed?.(newTitle);
      toast.success('Meeting renamed');
    } catch (error) {
      console.error('Failed to rename meeting:', error);
      toast.error('Failed to rename meeting', {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  }, [draftTitle, meetingId, onRenamed, title]);

  const confirmTrash = useCallback(async () => {
    setIsConfirmingTrash(false);

    try {
      // Soft-delete: the meeting moves to the trash (its transcripts/summary
      // stay intact) and is auto-purged after 30 days. Fully reversible.
      await invoke('api_delete_meeting', { meetingId });
      onTrashed?.();

      toast.success('Meeting moved to trash', {
        description: `"${title}" — kept for 30 days, then removed`,
        action: {
          label: 'Undo',
          onClick: async () => {
            try {
              await invoke('api_restore_meeting', { meetingId });
              onRestored?.();
              toast.success('Meeting restored');
            } catch (error) {
              console.error('Failed to restore meeting:', error);
              toast.error('Failed to restore meeting', {
                description: error instanceof Error ? error.message : String(error),
              });
            }
          },
        },
      });
    } catch (error) {
      console.error('Failed to delete meeting:', error);
      toast.error('Failed to delete meeting', {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  }, [meetingId, onRestored, onTrashed, title]);

  const dialogs = (
    <>
      <Dialog open={isRenaming} onOpenChange={(open) => !open && closeRename()}>
        <DialogContent
          className="sm:max-w-[425px]"
          onOpenAutoFocus={(e) => e.preventDefault()}
        >
          <DialogHeader>
            <DialogTitle>Rename meeting</DialogTitle>
          </DialogHeader>
          <Input
            ref={titleInputRef}
            aria-label="Meeting title"
            value={draftTitle}
            onChange={(e) => setDraftTitle(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                confirmRename();
              } else if (e.key === 'Escape') {
                closeRename();
              }
            }}
            placeholder="Meeting title"
          />
          <DialogFooter>
            <Button variant="ghost" onClick={closeRename}>
              Cancel
            </Button>
            <Button onClick={confirmRename}>Save</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmationModal
        isOpen={isConfirmingTrash}
        title="Move to trash"
        confirmLabel="Move to trash"
        text="Move this meeting to the trash? You can undo right after, and trashed meetings are automatically removed after 30 days."
        onConfirm={confirmTrash}
        onCancel={() => setIsConfirmingTrash(false)}
      />

      <MoveToProjectDialog
        open={isMovingToProject}
        onOpenChange={setIsMovingToProject}
        meetingIds={[meetingId]}
        currentProjectId={projectId}
        onMoved={onMovedToProject}
      />
    </>
  );

  return { openRename, openTrash, openMoveToProject, dialogs };
}

interface MeetingActionsDropdownProps {
  onRename: () => void;
  onTrash: () => void;
  /** Omit to hide the item (e.g. where a project context makes it redundant). */
  onMoveToProject?: () => void;
  align?: 'start' | 'end' | 'center';
  /** Extra classes for the kebab trigger. */
  className?: string;
}

/**
 * The kebab trigger plus Rename / Move-to-trash items, without any state. Use
 * this when the surrounding UI already holds a `useMeetingActions` instance
 * (e.g. a header whose title also opens the rename dialog).
 */
export function MeetingActionsDropdown({
  onRename,
  onTrash,
  onMoveToProject,
  align = 'end',
  className = '',
}: MeetingActionsDropdownProps) {
  // Both items open a dialog as the menu closes. Radix would otherwise restore
  // focus to the kebab and pull it out of the dialog's autofocused input.
  const handedOffToDialog = React.useRef(false);

  const select = (action: () => void) => () => {
    handedOffToDialog.current = true;
    action();
  };

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        // The row/card underneath is clickable; keep the menu from navigating.
        onClick={(e) => e.stopPropagation()}
        aria-label="Meeting actions"
        className={`flex-shrink-0 p-1 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent outline-none focus-visible:ring-2 focus-visible:ring-ring data-[state=open]:bg-accent data-[state=open]:text-foreground ${className}`}
      >
        <MoreVertical className="w-4 h-4" />
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align={align}
        className="w-44"
        onCloseAutoFocus={(e) => {
          if (handedOffToDialog.current) {
            handedOffToDialog.current = false;
            e.preventDefault();
          }
        }}
      >
        <DropdownMenuItem onSelect={select(onRename)}>
          <Pencil className="w-4 h-4" />
          Rename
        </DropdownMenuItem>
        {onMoveToProject && (
          <DropdownMenuItem onSelect={select(onMoveToProject)}>
            <FolderInput className="w-4 h-4" />
            Move to project
          </DropdownMenuItem>
        )}
        <DropdownMenuItem
          onSelect={select(onTrash)}
          className="text-destructive focus:text-destructive focus:bg-destructive/10"
        >
          <Trash2 className="w-4 h-4" />
          Move to trash
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export interface MeetingActionsMenuProps extends MeetingActionsOptions {
  align?: 'start' | 'end' | 'center';
  className?: string;
}

/** Self-contained kebab menu: the dropdown plus its rename/trash dialogs. */
export function MeetingActionsMenu({ align, className, ...options }: MeetingActionsMenuProps) {
  const { openRename, openTrash, openMoveToProject, dialogs } = useMeetingActions(options);

  return (
    <>
      <MeetingActionsDropdown
        onRename={openRename}
        onTrash={openTrash}
        onMoveToProject={openMoveToProject}
        align={align}
        className={className}
      />
      {dialogs}
    </>
  );
}
