'use client';

import { useState } from 'react';
import {
  ExternalLink,
  File,
  FileArchive,
  FileAudio,
  FileSpreadsheet,
  FileText,
  FileVideo,
  Info,
  Paperclip,
  Plus,
  Trash2,
} from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Attachment, attachmentUrl, formatBytes } from '@/lib/attachmentsApi';
import { UseAttachmentsResult } from '@/hooks/meeting-details/useAttachments';

interface AttachmentsPanelProps {
  attachmentsApi: UseAttachmentsResult;
  hasSummary: boolean;
}

function fileIconFor(mimeType: string) {
  if (mimeType.startsWith('audio/')) return FileAudio;
  if (mimeType.startsWith('video/')) return FileVideo;
  if (mimeType.startsWith('text/') || mimeType === 'application/pdf' || mimeType.includes('word') || mimeType.includes('presentation')) {
    return FileText;
  }
  if (mimeType.includes('spreadsheet') || mimeType === 'text/csv' || mimeType.includes('excel')) {
    return FileSpreadsheet;
  }
  if (mimeType === 'application/zip') return FileArchive;
  return File;
}

export function AttachmentsPanel({ attachmentsApi, hasSummary }: AttachmentsPanelProps) {
  const { attachments, isLoading, addViaPicker, remove, open, hasMutated } = attachmentsApi;
  const [lightbox, setLightbox] = useState<Attachment | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<Attachment | null>(null);

  const images = attachments.filter((a) => a.is_image);
  const otherFiles = attachments.filter((a) => !a.is_image);

  const handleConfirmDelete = async () => {
    if (!confirmDelete) return;
    const id = confirmDelete.id;
    setConfirmDelete(null);
    if (lightbox?.id === id) setLightbox(null);
    await remove(id);
  };

  return (
    <div className="flex flex-col h-full overflow-hidden">
      <div className="flex items-center justify-between gap-3 border-b border-border px-4 py-3">
        <div className="text-sm text-muted-foreground">
          {attachments.length === 0
            ? 'No attachments'
            : `${attachments.length} attachment${attachments.length === 1 ? '' : 's'}`}
          <span className="hidden md:inline"> · or drop files anywhere on this page</span>
        </div>
        <Button size="sm" onClick={addViaPicker}>
          <Plus /> Add files
        </Button>
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-6">
        {hasMutated && hasSummary && (
          <div className="flex items-start gap-2 rounded-md border border-border bg-muted/40 px-3 py-2 text-sm text-muted-foreground">
            <Info className="h-4 w-4 mt-0.5 shrink-0" />
            <span>
              Attachments changed — the summary may be out of date. Regenerate it from the Summary
              tab to include the new context.
            </span>
          </div>
        )}

        {isLoading && attachments.length === 0 && (
          <p className="text-sm text-muted-foreground">Loading attachments…</p>
        )}

        {!isLoading && attachments.length === 0 && (
          <div className="flex flex-col items-center justify-center gap-3 rounded-2xl border-2 border-dashed border-border p-12 text-center">
            <Paperclip className="h-10 w-10 text-muted-foreground" />
            <div>
              <p className="font-medium text-foreground">No attachments yet</p>
              <p className="text-sm text-muted-foreground mt-1">
                Add photos or files for context — images are shared with the AI when generating
                summaries and answering chat questions.
              </p>
            </div>
            <Button variant="outline" size="sm" onClick={addViaPicker}>
              <Plus /> Add files
            </Button>
          </div>
        )}

        {images.length > 0 && (
          <section>
            <h3 className="text-xs font-medium uppercase tracking-wide text-muted-foreground mb-2">
              Images
            </h3>
            <div className="grid grid-cols-[repeat(auto-fill,minmax(9rem,1fr))] gap-3">
              {images.map((attachment) => (
                <div
                  key={attachment.id}
                  className="group relative aspect-square overflow-hidden rounded-lg border border-border bg-muted/30"
                >
                  <button
                    type="button"
                    className="h-full w-full"
                    onClick={() => setLightbox(attachment)}
                    title={attachment.file_name}
                  >
                    {/* eslint-disable-next-line @next/next/no-img-element */}
                    <img
                      src={attachmentUrl(attachment)}
                      alt={attachment.file_name}
                      loading="lazy"
                      className="h-full w-full object-cover"
                    />
                  </button>
                  <div className="pointer-events-none absolute inset-x-0 bottom-0 bg-gradient-to-t from-black/60 to-transparent p-2 opacity-0 transition-opacity group-hover:opacity-100">
                    <p className="truncate text-xs text-white">{attachment.file_name}</p>
                  </div>
                  <div className="absolute right-1 top-1 flex gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                    <Button
                      variant="secondary"
                      size="icon"
                      className="h-7 w-7"
                      title="Open with system viewer"
                      onClick={() => open(attachment.id)}
                    >
                      <ExternalLink className="h-3.5 w-3.5" />
                    </Button>
                    <Button
                      variant="secondary"
                      size="icon"
                      className="h-7 w-7 hover:text-destructive"
                      title="Delete attachment"
                      onClick={() => setConfirmDelete(attachment)}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                </div>
              ))}
            </div>
          </section>
        )}

        {otherFiles.length > 0 && (
          <section>
            <h3 className="text-xs font-medium uppercase tracking-wide text-muted-foreground mb-2">
              Files
            </h3>
            <ul className="space-y-1">
              {otherFiles.map((attachment) => {
                const Icon = fileIconFor(attachment.mime_type);
                return (
                  <li
                    key={attachment.id}
                    className="group flex items-center gap-3 rounded-md border border-border px-3 py-2 hover:bg-accent/50"
                  >
                    <Icon className="h-5 w-5 shrink-0 text-muted-foreground" />
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-sm text-foreground" title={attachment.file_name}>
                        {attachment.file_name}
                      </p>
                      <p className="text-xs text-muted-foreground">
                        {formatBytes(attachment.size_bytes)} ·{' '}
                        {new Date(attachment.created_at).toLocaleDateString()}
                      </p>
                    </div>
                    <div className="flex gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-8 w-8"
                        title="Open with system app"
                        onClick={() => open(attachment.id)}
                      >
                        <ExternalLink className="h-4 w-4" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-8 w-8 hover:text-destructive"
                        title="Delete attachment"
                        onClick={() => setConfirmDelete(attachment)}
                      >
                        <Trash2 className="h-4 w-4" />
                      </Button>
                    </div>
                  </li>
                );
              })}
            </ul>
          </section>
        )}
      </div>

      {/* Image lightbox */}
      <Dialog open={lightbox !== null} onOpenChange={(open) => !open && setLightbox(null)}>
        <DialogContent className="max-w-4xl">
          {lightbox && (
            <>
              <DialogHeader>
                <DialogTitle className="truncate pr-8">{lightbox.file_name}</DialogTitle>
                <DialogDescription>
                  {lightbox.mime_type} · {formatBytes(lightbox.size_bytes)}
                </DialogDescription>
              </DialogHeader>
              {/* eslint-disable-next-line @next/next/no-img-element */}
              <img
                src={attachmentUrl(lightbox)}
                alt={lightbox.file_name}
                className="mx-auto max-h-[70vh] w-auto max-w-full rounded-md object-contain"
              />
              <DialogFooter>
                <Button variant="outline" onClick={() => open(lightbox.id)}>
                  <ExternalLink /> Open
                </Button>
                <Button variant="destructive" onClick={() => setConfirmDelete(lightbox)}>
                  <Trash2 /> Delete
                </Button>
              </DialogFooter>
            </>
          )}
        </DialogContent>
      </Dialog>

      {/* Delete confirmation */}
      <Dialog
        open={confirmDelete !== null}
        onOpenChange={(open) => !open && setConfirmDelete(null)}
      >
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>Delete attachment?</DialogTitle>
            <DialogDescription>
              “{confirmDelete?.file_name}” will be removed from this meeting and its copied file
              deleted. The original file on your disk is not affected.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setConfirmDelete(null)}>
              Cancel
            </Button>
            <Button variant="destructive" onClick={handleConfirmDelete}>
              Delete
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
