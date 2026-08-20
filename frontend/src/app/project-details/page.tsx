'use client';

import React, { Suspense, useCallback, useEffect, useState } from 'react';
import { useRouter, useSearchParams } from 'next/navigation';
import {
  ArrowLeft,
  Calendar,
  Clock,
  Download,
  FileText,
  FolderKanban,
  List,
  LoaderIcon,
  MessageSquare,
  NotebookPen,
  Pencil,
  Plus,
  Timer,
  Trash2,
  X,
} from 'lucide-react';
import { toast } from 'sonner';

import { Button } from '@/components/ui/button';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { ConfirmationModal } from '@/components/ConfirmationModal/confirmation-modal';
import { MeetingActionsMenu } from '@/components/MeetingActions/MeetingActionsMenu';
import { AddMeetingsDialog } from '@/components/Projects/AddMeetingsDialog';
import { ExportBundleDialog } from '@/components/Export/ExportBundleDialog';
import { ProjectFormDialog } from '@/components/Projects/ProjectFormDialog';
import { ProjectChatPanel } from '@/components/Projects/ProjectChatPanel';
import { ProjectNotesPanel } from '@/components/Projects/ProjectNotesPanel';
import { ProjectSummaryPanel } from '@/components/Projects/ProjectSummaryPanel';
import { ModelPicker } from '@/components/chat/ModelPicker';
import { useChatModelSelection } from '@/hooks/useChatModelSelection';
import { useProjectSummary } from '@/hooks/projects/useProjectSummary';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { formatMeetingDuration } from '@/lib/meetingDuration';
import { projectClasses } from '@/lib/projectColors';
import {
  Project,
  ProjectMeeting,
  assignMeetingsToProject,
  deleteProject,
  getProject,
  getProjectMeetings,
  notifyProjectsChanged,
  onProjectsChanged,
} from '@/lib/projectsApi';

type ProjectTab = 'meetings' | 'summary' | 'chat' | 'notes';

function parseDate(value?: string): Date | null {
  if (!value) return null;
  const date = new Date(value);
  return isNaN(date.getTime()) ? null : date;
}

function formatDate(value?: string): string {
  const date = parseDate(value);
  return date
    ? date.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' })
    : '';
}

function formatTime(value?: string): string {
  const date = parseDate(value);
  return date ? date.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' }) : '';
}

function ProjectDetailsContent() {
  const router = useRouter();
  const searchParams = useSearchParams();
  const projectId = searchParams.get('id');
  const { refetchMeetings } = useSidebar();

  const [project, setProject] = useState<Project | null>(null);
  const [meetings, setMeetings] = useState<ProjectMeeting[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isEditing, setIsEditing] = useState(false);
  const [isAdding, setIsAdding] = useState(false);
  const [isExporting, setIsExporting] = useState(false);
  const [isConfirmingDelete, setIsConfirmingDelete] = useState(false);

  // Deep-linkable, but not written back to the URL: a router.replace on every
  // tab click would churn useSearchParams and re-render the whole subtree.
  const [tab, setTab] = useState<ProjectTab>(() => {
    const requested = searchParams.get('tab');
    return requested === 'summary' || requested === 'chat' || requested === 'notes'
      ? requested
      : 'meetings';
  });

  const modelSelection = useChatModelSelection();

  // Hoisted above <Tabs> deliberately: Radix unmounts the inactive TabsContent,
  // so a poller owned by the Summary panel would be torn down the moment the
  // user glanced at Chat, and a running generation would go unnoticed.
  const projectSummary = useProjectSummary({
    projectId: projectId ?? '',
    provider: modelSelection.provider,
    model: modelSelection.model,
  });

  /**
   * Reload the project and its meetings.
   *
   * Deliberately never sets `project` or `meetings` back to null, and never
   * gates on an `isRefreshing` flag: `onProjectsChanged` fires on any project
   * mutation anywhere in the app, and the loader below is a full-screen early
   * return. Blanking here would unmount the tabs, an in-flight chat send, and
   * any open dialog every time someone renamed a project in another window.
   */
  const load = useCallback(async () => {
    if (!projectId) {
      setError('No project selected');
      return;
    }
    try {
      const [loadedProject, loadedMeetings] = await Promise.all([
        getProject(projectId),
        getProjectMeetings(projectId),
      ]);
      setProject(loadedProject);
      setMeetings(loadedMeetings);
      setError(null);
    } catch (e) {
      console.error('Failed to load project:', e);
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [projectId]);

  useEffect(() => {
    load();
  }, [load]);

  // Another view may move a meeting in or out of this project.
  useEffect(() => onProjectsChanged(load), [load]);

  // Removing only unfiles the meeting — it stays in Meetings. No confirmation
  // for that; it is one click to undo from the same dialogs.
  const removeMeeting = async (meeting: ProjectMeeting) => {
    try {
      await assignMeetingsToProject([meeting.id], null);
      setMeetings((prev) => (prev ?? []).filter((m) => m.id !== meeting.id));
      setProject((prev) =>
        prev ? { ...prev, meetingCount: Math.max(0, prev.meetingCount - 1) } : prev,
      );
      notifyProjectsChanged();
      toast.success('Removed from project', { description: `"${meeting.title}" is now unfiled.` });
    } catch (e) {
      console.error('Failed to remove meeting from project:', e);
      toast.error('Failed to remove meeting', {
        description: e instanceof Error ? e.message : String(e),
      });
    }
  };

  const confirmDelete = async () => {
    if (!project) return;
    setIsConfirmingDelete(false);
    try {
      await deleteProject(project.id);
      notifyProjectsChanged();
      toast.success('Project deleted', {
        description: 'Its meetings are still in Meetings, just unfiled.',
      });
      router.push('/projects');
    } catch (e) {
      console.error('Failed to delete project:', e);
      toast.error('Failed to delete project', {
        description: e instanceof Error ? e.message : String(e),
      });
    }
  };

  if (error) {
    return (
      <div className="h-[calc(100vh-var(--titlebar-height))] flex flex-col items-center justify-center gap-3 text-center">
        <FolderKanban className="w-10 h-10 text-muted-foreground opacity-40" />
        <p className="text-lg">Project unavailable</p>
        <p className="text-sm text-muted-foreground max-w-sm">{error}</p>
        <Button variant="ghost" onClick={() => router.push('/projects')}>
          <ArrowLeft className="w-4 h-4" />
          Back to projects
        </Button>
      </div>
    );
  }

  if (!project) {
    return (
      <div className="flex items-center justify-center h-[calc(100vh-var(--titlebar-height))]">
        <LoaderIcon className="animate-spin size-6" />
      </div>
    );
  }

  return (
    <div className="h-[calc(100vh-var(--titlebar-height))] bg-background flex flex-col">
      <Tabs
        value={tab}
        onValueChange={(v) => setTab(v as ProjectTab)}
        className="flex flex-1 min-h-0 flex-col"
      >
        {/* `shrink-0` rather than `sticky`: the page is no longer one scroll
            region, because the chat has to pin its composer to the bottom of a
            box that does not scroll. Each tab owns its own scrolling. */}
        <div className="shrink-0 bg-background/80 backdrop-blur border-b border-border">
        <div className="max-w-4xl mx-auto px-4 md:px-8 pt-6">
          <button
            onClick={() => router.push('/projects')}
            className="flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground transition-colors"
          >
            <ArrowLeft className="w-4 h-4" />
            Projects
          </button>
          <div className="mt-3 flex flex-col sm:flex-row sm:items-start gap-3">
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-3">
                <FolderKanban
                  className={`w-6 h-6 shrink-0 ${projectClasses(project).text}`}
                />
                <h1 className="text-3xl font-bold truncate">{project.name}</h1>
              </div>
              {project.description && (
                <p className="mt-2 text-sm text-muted-foreground whitespace-pre-line">
                  {project.description}
                </p>
              )}
              <p className="mt-2 text-sm text-muted-foreground">
                {project.meetingCount} {project.meetingCount === 1 ? 'meeting' : 'meetings'}
              </p>
            </div>
            <div className="flex items-center gap-2 shrink-0">
              <Button onClick={() => setIsAdding(true)}>
                <Plus className="w-4 h-4" />
                Add meetings
              </Button>
              <Button
                variant="ghost"
                onClick={() => setIsExporting(true)}
                aria-label="Export project"
                disabled={project.meetingCount === 0}
                title={
                  project.meetingCount === 0
                    ? 'No meetings to export'
                    : 'Export this project as a ZIP'
                }
              >
                <Download className="w-4 h-4" />
              </Button>
              <Button variant="ghost" onClick={() => setIsEditing(true)} aria-label="Edit project">
                <Pencil className="w-4 h-4" />
              </Button>
              <Button
                variant="ghost"
                onClick={() => setIsConfirmingDelete(true)}
                aria-label="Delete project"
                className="text-destructive hover:text-destructive hover:bg-destructive/10"
              >
                <Trash2 className="w-4 h-4" />
              </Button>
            </div>
          </div>

          <TabsList className="mt-4 mb-3">
            <TabsTrigger value="meetings" className="gap-1.5">
              <List className="h-4 w-4" /> Meetings
              {project.meetingCount > 0 && (
                <span className="rounded-full bg-muted px-1.5 text-xs text-muted-foreground">
                  {project.meetingCount}
                </span>
              )}
            </TabsTrigger>
            <TabsTrigger value="summary" className="gap-1.5">
              <FileText className="h-4 w-4" /> Summary
              {projectSummary.isGenerating ? (
                <LoaderIcon className="h-3 w-3 animate-spin" />
              ) : (
                projectSummary.summary?.coverage?.isStale && (
                  // Pulls the user back at exactly the moment the brief became
                  // wrong — right after they added or changed a meeting.
                  <span
                    aria-label="Brief is out of date"
                    className="h-1.5 w-1.5 rounded-full bg-amber-500"
                  />
                )
              )}
            </TabsTrigger>
            <TabsTrigger value="chat" className="gap-1.5">
              <MessageSquare className="h-4 w-4" /> Chat
            </TabsTrigger>
            <TabsTrigger value="notes" className="gap-1.5">
              <NotebookPen className="h-4 w-4" /> Notes
              {project.contextNotes && (
                <span
                  aria-label="This project has notes"
                  className="h-1.5 w-1.5 rounded-full bg-brand"
                />
              )}
            </TabsTrigger>
          </TabsList>
        </div>
        </div>

        <TabsContent value="meetings" className="mt-0 flex-1 min-h-0 overflow-y-auto">
        <div className="max-w-4xl mx-auto p-4 md:p-8">
          {meetings === null ? (
            <div className="text-muted-foreground">Loading…</div>
          ) : meetings.length === 0 ? (
            <div className="flex flex-col items-center justify-center py-20 text-center text-muted-foreground">
              <FolderKanban className="w-10 h-10 mb-3 opacity-40" />
              <p className="text-lg">No meetings in this project</p>
              <p className="text-sm">Add existing meetings to keep them together.</p>
              <Button onClick={() => setIsAdding(true)} className="mt-4">
                <Plus className="w-4 h-4" />
                Add meetings
              </Button>
            </div>
          ) : (
            <ul className="space-y-1.5">
              {meetings.map((meeting) => {
                const duration = formatMeetingDuration(meeting.durationSeconds);

                return (
                  <li key={meeting.id}>
                    <div className="group relative overflow-hidden rounded-lg border border-border bg-card hover:bg-accent transition-colors">
                      <span
                        aria-hidden
                        className={`absolute inset-y-0 left-0 w-1 ${projectClasses(project).solid}`}
                      />
                      <button
                        onClick={() => router.push(`/meeting-details?id=${meeting.id}`)}
                        className="w-full text-left p-4 pl-5 pr-20"
                      >
                        <div className="flex items-center gap-4">
                          <div className="flex-1 min-w-0">
                            <p className="font-medium truncate">{meeting.title}</p>
                          </div>
                          <div className="shrink-0 flex items-center gap-4 text-xs text-muted-foreground">
                            <span className="flex items-center gap-1.5">
                              <Calendar className="w-3.5 h-3.5" />
                              {formatDate(meeting.createdAt)}
                            </span>
                            <span className="flex items-center gap-1.5">
                              <Clock className="w-3.5 h-3.5" />
                              {formatTime(meeting.createdAt)}
                            </span>
                            {duration && (
                              <span
                                className="flex items-center gap-1.5 tabular-nums"
                                title="Recording length"
                              >
                                <Timer className="w-3.5 h-3.5" />
                                {duration}
                              </span>
                            )}
                          </div>
                        </div>
                      </button>
                      <div className="absolute right-2 top-3.5 flex items-center gap-1">
                        <button
                          onClick={() => removeMeeting(meeting)}
                          aria-label="Remove from project"
                          title="Remove from project"
                          className="p-1 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent outline-none focus-visible:ring-2 focus-visible:ring-ring"
                        >
                          <X className="w-4 h-4" />
                        </button>
                        <MeetingActionsMenu
                          meetingId={meeting.id}
                          title={meeting.title}
                          onRenamed={(newTitle) => {
                            setMeetings((prev) =>
                              (prev ?? []).map((m) =>
                                m.id === meeting.id ? { ...m, title: newTitle } : m,
                              ),
                            );
                            refetchMeetings();
                          }}
                          onTrashed={() => {
                            setMeetings((prev) => (prev ?? []).filter((m) => m.id !== meeting.id));
                            setProject((prev) =>
                              prev ? { ...prev, meetingCount: Math.max(0, prev.meetingCount - 1) } : prev,
                            );
                            refetchMeetings();
                          }}
                          onRestored={async () => {
                            await load();
                            await refetchMeetings();
                          }}
                        />
                      </div>
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
        </TabsContent>

        <TabsContent value="summary" className="mt-0 flex-1 min-h-0 overflow-hidden">
          <ProjectSummaryPanel
            meetingCount={project.meetingCount}
            summary={projectSummary.summary}
            isLoading={projectSummary.isLoading}
            isGenerating={projectSummary.isGenerating}
            onGenerate={projectSummary.generate}
            onCancel={projectSummary.cancel}
            onAddMeetings={() => setIsAdding(true)}
            hasModel={Boolean(modelSelection.model)}
            modelPicker={
              <ModelPicker
                provider={modelSelection.provider}
                model={modelSelection.model}
                ollamaModels={modelSelection.ollamaModelNames}
                modelOptions={modelSelection.modelOptions}
                providerApiKeys={modelSelection.providerApiKeys}
                chatgptSignedIn={modelSelection.chatgptSignedIn}
                onOpen={modelSelection.onPickerOpen}
                onPick={modelSelection.handlePickModel}
              />
            }
          />
        </TabsContent>

        <TabsContent value="chat" className="mt-0 flex-1 min-h-0 overflow-hidden">
          <ProjectChatPanel
            projectId={project.id}
            meetingCount={project.meetingCount}
            context={projectSummary.summary?.meetings ?? null}
          />
        </TabsContent>

        <TabsContent value="notes" className="mt-0 flex-1 min-h-0 overflow-hidden">
          <ProjectNotesPanel
            projectId={project.id}
            notes={project.contextNotes}
            onSaved={(contextNotes) =>
              setProject((prev) => (prev ? { ...prev, contextNotes } : prev))
            }
          />
        </TabsContent>
      </Tabs>

      {/* Dialogs live OUTSIDE <Tabs>: inside a TabsContent, switching tabs would
          unmount an open dialog, the same failure mode as a full-screen loader. */}
      <ProjectFormDialog
        open={isEditing}
        onOpenChange={setIsEditing}
        project={project}
        onSaved={setProject}
      />

      <ExportBundleDialog
        open={isExporting}
        onOpenChange={setIsExporting}
        target={{ kind: 'project', projectId: project.id, name: project.name }}
      />

      <AddMeetingsDialog
        open={isAdding}
        onOpenChange={setIsAdding}
        projectId={project.id}
        projectName={project.name}
        onAdded={load}
      />

      <ConfirmationModal
        isOpen={isConfirmingDelete}
        title="Delete project"
        confirmLabel="Delete project"
        text={`Delete "${project.name}"? Its ${project.meetingCount} ${
          project.meetingCount === 1 ? 'meeting' : 'meetings'
        } will stay in Meetings — only the project grouping goes away.`}
        onConfirm={confirmDelete}
        onCancel={() => setIsConfirmingDelete(false)}
      />
    </div>
  );
}

export default function ProjectDetails() {
  return (
    <Suspense
      fallback={
        <div className="flex items-center justify-center h-[calc(100vh-var(--titlebar-height))]">
          <LoaderIcon className="animate-spin size-6" />
        </div>
      }
    >
      <ProjectDetailsContent />
    </Suspense>
  );
}
