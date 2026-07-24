'use client';

import { useCallback, useEffect, useState } from 'react';
import { toast } from 'sonner';
import {
  Attachment,
  addAttachments,
  addAttachmentsFromPaths,
  deleteAttachment,
  listAttachments,
  openAttachment,
} from '@/lib/attachmentsApi';

export interface UseAttachmentsResult {
  attachments: Attachment[];
  isLoading: boolean;
  addViaPicker: () => Promise<void>;
  addFromPaths: (paths: string[]) => Promise<void>;
  remove: (attachmentId: string) => Promise<void>;
  open: (attachmentId: string) => Promise<void>;
  /** True once this session has added or removed an attachment (summary staleness hint). */
  hasMutated: boolean;
}

export function useAttachments(meetingId: string | null): UseAttachmentsResult {
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [hasMutated, setHasMutated] = useState(false);

  useEffect(() => {
    setAttachments([]);
    setHasMutated(false);
    if (!meetingId) return;

    // invoke() is not abortable — discard stale results manually.
    let stale = false;
    setIsLoading(true);
    listAttachments(meetingId)
      .then((items) => {
        if (!stale) setAttachments(items);
      })
      .catch((error) => {
        console.error('[useAttachments] Failed to load attachments:', error);
        if (!stale) {
          toast.error('Failed to load attachments', { description: String(error) });
        }
      })
      .finally(() => {
        if (!stale) setIsLoading(false);
      });

    return () => {
      stale = true;
    };
  }, [meetingId]);

  const appendAdded = useCallback((added: Attachment[]) => {
    if (added.length === 0) return;
    setAttachments((prev) => [...prev, ...added]);
    setHasMutated(true);
    toast.success(added.length === 1 ? 'Attachment added' : `${added.length} attachments added`);
  }, []);

  const addViaPicker = useCallback(async () => {
    if (!meetingId) return;
    try {
      appendAdded(await addAttachments(meetingId));
    } catch (error) {
      console.error('[useAttachments] Failed to add attachments:', error);
      toast.error('Failed to add attachments', { description: String(error) });
    }
  }, [meetingId, appendAdded]);

  const addFromPaths = useCallback(
    async (paths: string[]) => {
      if (!meetingId || paths.length === 0) return;
      try {
        appendAdded(await addAttachmentsFromPaths(meetingId, paths));
      } catch (error) {
        console.error('[useAttachments] Failed to attach dropped files:', error);
        toast.error('Failed to attach dropped files', { description: String(error) });
      }
    },
    [meetingId, appendAdded],
  );

  const remove = useCallback(
    async (attachmentId: string) => {
      // Optimistic removal with rollback on error.
      let removed: Attachment | undefined;
      setAttachments((prev) => {
        removed = prev.find((a) => a.id === attachmentId);
        return prev.filter((a) => a.id !== attachmentId);
      });
      try {
        await deleteAttachment(attachmentId);
        setHasMutated(true);
        toast.success('Attachment deleted');
      } catch (error) {
        console.error('[useAttachments] Failed to delete attachment:', error);
        const rollback = removed;
        if (rollback) setAttachments((prev) => [...prev, rollback]);
        toast.error('Failed to delete attachment', { description: String(error) });
      }
    },
    [],
  );

  const open = useCallback(async (attachmentId: string) => {
    try {
      await openAttachment(attachmentId);
    } catch (error) {
      console.error('[useAttachments] Failed to open attachment:', error);
      toast.error('Failed to open attachment', { description: String(error) });
    }
  }, []);

  return { attachments, isLoading, addViaPicker, addFromPaths, remove, open, hasMutated };
}
