import { invoke, convertFileSrc } from '@tauri-apps/api/core';

export interface Attachment {
  id: string;
  meeting_id: string;
  file_name: string;
  mime_type: string;
  size_bytes: number;
  created_at: string;
  is_image: boolean;
  absolute_path: string;
}

/** Opens the native multi-select picker; resolves to [] when the user cancels. */
export async function addAttachments(meetingId: string): Promise<Attachment[]> {
  return invoke<Attachment[]>('api_add_attachments', { meetingId });
}

export async function addAttachmentsFromPaths(
  meetingId: string,
  paths: string[],
): Promise<Attachment[]> {
  return invoke<Attachment[]>('api_add_attachments_from_paths', { meetingId, paths });
}

export async function listAttachments(meetingId: string): Promise<Attachment[]> {
  return invoke<Attachment[]>('api_list_attachments', { meetingId });
}

export async function deleteAttachment(attachmentId: string): Promise<void> {
  await invoke('api_delete_attachment', { attachmentId });
}

export async function openAttachment(attachmentId: string): Promise<void> {
  await invoke('api_open_attachment', { attachmentId });
}

/**
 * Display URL for an image attachment. Attachments live under the app data dir,
 * which is inside the asset-protocol scope, so no byte copy over IPC is needed.
 */
export function attachmentUrl(attachment: Attachment): string {
  return convertFileSrc(attachment.absolute_path);
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
