import { invoke } from '@tauri-apps/api/core';

/**
 * Export a meeting, or a chosen subset of a project's meetings, as a .zip.
 *
 * Everything heavy happens in Rust: it reads the DB, renders the markdown,
 * opens the native save dialog and streams the archive. Attachment and audio
 * bytes never cross the IPC boundary — Tauri serializes `Vec<u8>` as a JSON
 * number array, so a project with 200 MB of files would become a ~1 GB payload.
 */
export type ExportBundleScope =
  | { kind: 'meeting'; meetingId: string }
  | { kind: 'project'; projectId: string; meetingIds: string[] };

export interface ExportContents {
  transcript: boolean;
  summary: boolean;
  attachments: boolean;
  /** Off by default — an hour of audio is 50-100 MB. */
  audio: boolean;
}

export interface TranscriptFormat {
  includeTimestamps: boolean;
  includeSpeakers: boolean;
}

export interface ExportBundleResult {
  /** null when the user dismissed the native save dialog. */
  path: string | null;
  meetingsExported: number;
  filesWritten: number;
  bytesWritten: number;
  /** Non-fatal problems, e.g. an attachment whose file is gone. */
  warnings: string[];
}

/** What one meeting can contribute to an export. */
export interface MeetingExportInfo {
  meetingId: string;
  title: string;
  createdAt: string;
  transcriptSegments: number;
  hasSummary: boolean;
  attachmentCount: number;
  attachmentBytes: number;
  /** null when the meeting has no recording on disk. */
  audioBytes: number | null;
}

export interface ProjectExportInfo {
  id: string;
  name: string;
  description: string | null;
}

export interface ExportAvailability {
  /** Present only for project scope. */
  project: ProjectExportInfo | null;
  /** One entry for meeting scope; every live meeting for project scope. */
  meetings: MeetingExportInfo[];
}

export const DEFAULT_EXPORT_CONTENTS: ExportContents = {
  transcript: true,
  summary: true,
  attachments: true,
  audio: false,
};

export const DEFAULT_TRANSCRIPT_FORMAT: TranscriptFormat = {
  includeTimestamps: false,
  includeSpeakers: true,
};

/**
 * What the dialog can offer, in one round trip.
 *
 * Sizes come from stat-ing the filesystem, so this has to be a Rust call; and
 * doing it in one command keeps a 14-meeting project at 1 invoke instead of 42.
 */
export async function fetchExportAvailability(
  scope: ExportBundleScope,
): Promise<ExportAvailability> {
  return invoke<ExportAvailability>('export_bundle_availability', { scope });
}

export async function exportBundle(
  scope: ExportBundleScope,
  contents: ExportContents,
  transcriptFormat: TranscriptFormat,
): Promise<ExportBundleResult> {
  return invoke<ExportBundleResult>('export_bundle', {
    request: { scope, contents, transcriptFormat },
  });
}

/** Compact size label for the include rows ("86.4 MB"). */
export function formatExportBytes(bytes: number): string {
  if (bytes <= 0) return '0 B';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}
