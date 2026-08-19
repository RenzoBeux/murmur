import { invoke } from '@tauri-apps/api/core';

/**
 * A project is a named folder of meetings. A meeting belongs to at most one
 * project (`projectId` on the meeting row); `null` means unfiled.
 */
export interface Project {
  id: string;
  name: string;
  description: string | null;
  /**
   * Palette slug (see `projectColors.ts`), or null for projects created before
   * the picker existed — the UI derives a stable color from the id then.
   */
  color: string | null;
  createdAt: string;
  updatedAt: string;
  /** Live (non-trashed) meetings filed under this project. */
  meetingCount: number;
}

/** A meeting row as returned by the meeting/project list commands. */
export interface ProjectMeeting {
  id: string;
  title: string;
  createdAt?: string;
  projectId?: string | null;
}

// The Rust `Meeting` payload keeps snake_case for the fields that predate
// projects, so map it once here instead of at every call site.
interface RawMeeting {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  project_id: string | null;
}

function toMeeting(raw: RawMeeting): ProjectMeeting {
  return { id: raw.id, title: raw.title, createdAt: raw.created_at, projectId: raw.project_id };
}

export async function listProjects(): Promise<Project[]> {
  return invoke<Project[]>('api_list_projects');
}

export async function getProject(projectId: string): Promise<Project> {
  return invoke<Project>('api_get_project', { projectId });
}

export async function createProject(
  name: string,
  description?: string | null,
  color?: string | null,
): Promise<Project> {
  return invoke<Project>('api_create_project', {
    name,
    description: description ?? null,
    color: color ?? null,
  });
}

export async function updateProject(
  projectId: string,
  name: string,
  description?: string | null,
  color?: string | null,
): Promise<Project> {
  return invoke<Project>('api_update_project', {
    projectId,
    name,
    description: description ?? null,
    color: color ?? null,
  });
}

/** Deletes the project; its meetings survive, unfiled. */
export async function deleteProject(projectId: string): Promise<void> {
  await invoke('api_delete_project', { projectId });
}

export async function getProjectMeetings(projectId: string): Promise<ProjectMeeting[]> {
  const rows = await invoke<RawMeeting[]>('api_get_project_meetings', { projectId });
  return rows.map(toMeeting);
}

export async function listAllMeetings(): Promise<ProjectMeeting[]> {
  const rows = await invoke<RawMeeting[]>('api_get_meetings');
  return rows.map(toMeeting);
}

/**
 * Moves meetings into `projectId`, or unfiles them when it is null. Returns the
 * number of meetings that changed.
 */
export async function assignMeetingsToProject(
  meetingIds: string[],
  projectId: string | null,
): Promise<number> {
  return invoke<number>('api_assign_meetings_to_project', { meetingIds, projectId });
}

export async function getMeetingProject(meetingId: string): Promise<Project | null> {
  return invoke<Project | null>('api_get_meeting_project', { meetingId });
}

/**
 * Fired after any project mutation so open views (the projects page, the
 * sidebar, a meeting header) refresh without a reload. Mirrors the existing
 * `meetings-changed` event.
 */
export const PROJECTS_CHANGED_EVENT = 'projects-changed';

export function notifyProjectsChanged(): void {
  window.dispatchEvent(new CustomEvent(PROJECTS_CHANGED_EVENT));
}

export function onProjectsChanged(handler: () => void): () => void {
  window.addEventListener(PROJECTS_CHANGED_EVENT, handler);
  return () => window.removeEventListener(PROJECTS_CHANGED_EVENT, handler);
}
