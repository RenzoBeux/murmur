'use client';

import React, { useState, useEffect } from 'react';
import { ChevronDown, ChevronRight, File, Settings, PanelLeftClose, PanelLeftOpen, Calendar, Home, Trash2, Mic, Square, Plus, NotebookPen, Upload, List, FolderKanban } from 'lucide-react';
import { useRouter, usePathname } from 'next/navigation';
import { motion } from 'framer-motion';
import { getVersion } from '@tauri-apps/api/app';
import { useSidebar } from './SidebarProvider';
import type { CurrentMeeting } from '@/components/Sidebar/SidebarProvider';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useImportDialog } from '@/contexts/ImportDialogContext';
import { useConfig } from '@/contexts/ConfigContext';
import { GlobalEgressIndicator } from '@/components/GlobalEgressIndicator';
import { MeetingActionsMenu } from '@/components/MeetingActions/MeetingActionsMenu';
import { groupMeetingsByDate } from '@/lib/meetingGrouping';

import { MessageToast } from '../MessageToast';
import Info from '../Info';
import { ComplianceNotification } from '../ComplianceNotification';

interface SidebarItem {
  id: string;
  title: string;
  type: 'folder' | 'file';
  children?: SidebarItem[];
  // Populated for file items from the meeting's created_at (SidebarProvider).
  // Required by groupMeetingsByDate<T extends { createdAt?: string | null }>.
  createdAt?: string;
}

const Sidebar: React.FC = () => {
  const router = useRouter();
  const pathname = usePathname();
  const {
    currentMeeting,
    setCurrentMeeting,
    sidebarItems,
    isCollapsed,
    toggleCollapse,
    handleRecordingToggle,
    setMeetings,
    refetchMeetings
  } = useSidebar();

  // Get recording state from RecordingStateContext (single source of truth)
  const { isRecording } = useRecordingState();
  const { openImportDialog } = useImportDialog();
  const { betaFeatures } = useConfig();
  const [expandedFolders, setExpandedFolders] = useState<Set<string>>(new Set(['meetings']));

  // Ensure 'meetings' folder is always expanded
  useEffect(() => {
    if (!expandedFolders.has('meetings')) {
      const newExpanded = new Set(expandedFolders);
      newExpanded.add('meetings');
      setExpandedFolders(newExpanded);
    }
  }, [expandedFolders]);


  const [appVersion, setAppVersion] = useState<string>('');
  useEffect(() => {
    // Unavailable in browser preview — just hide the version line there
    getVersion().then(setAppVersion).catch(() => {});
  }, []);

  // The rename/trash calls themselves live in MeetingActionsMenu; the sidebar
  // only reconciles its own list afterwards. Both use functional updaters so
  // overlapping in-flight actions build on the latest list instead of a stale
  // render-time snapshot (which could resurrect a peer).
  const handleMeetingRenamed = (meetingId: string, newTitle: string) => {
    setMeetings((prev) =>
      prev.map((m: CurrentMeeting) => (m.id === meetingId ? { ...m, title: newTitle } : m))
    );

    if (currentMeeting?.id === meetingId) {
      setCurrentMeeting({ id: meetingId, title: newTitle });
    }
  };

  const handleMeetingTrashed = (meetingId: string) => {
    setMeetings((prev) => prev.filter((m: CurrentMeeting) => m.id !== meetingId));

    // If the active meeting was trashed, navigate home.
    if (currentMeeting?.id === meetingId) {
      setCurrentMeeting({ id: 'intro-call', title: '+ New Call' });
      router.push('/');
    }
  };

  const toggleFolder = (folderId: string) => {
    // Normal toggle behavior for all folders
    const newExpanded = new Set(expandedFolders);
    if (newExpanded.has(folderId)) {
      newExpanded.delete(folderId);
    } else {
      newExpanded.add(folderId);
    }
    setExpandedFolders(newExpanded);
  };

  const renderCollapsedIcons = () => {
    if (!isCollapsed) return null;

    const isHomePage = pathname === '/';
    const isSettingsPage = pathname === '/settings';
    const isTrashPage = pathname === '/trash';
    const isMeetingsListPage = pathname === '/meetings';
    // A project's own page belongs to the same nav entry as the projects list.
    const isProjectsPage = pathname === '/projects' || pathname === '/project-details';

    return (
      <TooltipProvider>
        <div className="flex flex-col items-center space-y-4 mt-4">
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={toggleCollapse}
                aria-label="Expand sidebar"
                className="p-2 rounded-lg text-muted-foreground hover:bg-accent hover:text-foreground transition-colors duration-150"
              >
                <PanelLeftOpen className="w-5 h-5" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              <p>Expand sidebar</p>
            </TooltipContent>
          </Tooltip>

          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={() => router.push('/')}
                className={`p-2 rounded-lg transition-colors duration-150 ${isHomePage ? 'bg-accent text-foreground' : 'text-muted-foreground hover:bg-accent hover:text-foreground'
                  }`}
              >
                <Home className="w-5 h-5" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              <p>Home</p>
            </TooltipContent>
          </Tooltip>

          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={() => router.push('/meetings')}
                className={`p-2 rounded-lg transition-colors duration-150 ${isMeetingsListPage ? 'bg-accent text-foreground' : 'text-muted-foreground hover:bg-accent hover:text-foreground'
                  }`}
              >
                <List className="w-5 h-5" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              <p>Meetings</p>
            </TooltipContent>
          </Tooltip>

          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={() => router.push('/projects')}
                className={`p-2 rounded-lg transition-colors duration-150 ${isProjectsPage ? 'bg-accent text-foreground' : 'text-muted-foreground hover:bg-accent hover:text-foreground'
                  }`}
              >
                <FolderKanban className="w-5 h-5" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              <p>Projects</p>
            </TooltipContent>
          </Tooltip>

          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={handleRecordingToggle}
                disabled={isRecording}
                className={`p-2 ${isRecording ? 'bg-destructive cursor-not-allowed shadow-glow-destructive' : 'bg-destructive hover:bg-destructive/90'} rounded-full transition-colors duration-150`}
              >
                {isRecording ? (
                  <Square className="w-5 h-5 text-destructive-foreground" />
                ) : (
                  <Mic className="w-5 h-5 text-destructive-foreground" />
                )}
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              <p>{isRecording ? "Recording in progress..." : "Start Recording"}</p>
            </TooltipContent>
          </Tooltip>

          {betaFeatures.importAndRetranscribe && (
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={() => openImportDialog()}
                  className="p-2 rounded-lg transition-colors duration-150 bg-brand/10 hover:bg-brand/20"
                >
                  <Upload className="w-5 h-5 text-brand" />
                </button>
              </TooltipTrigger>
              <TooltipContent side="right">
                <p>Import Audio</p>
              </TooltipContent>
            </Tooltip>
          )}

          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={() => router.push('/trash')}
                className={`p-2 rounded-lg transition-colors duration-150 ${isTrashPage ? 'bg-accent text-foreground' : 'text-muted-foreground hover:bg-accent hover:text-foreground'
                  }`}
              >
                <Trash2 className="w-5 h-5" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              <p>Trash</p>
            </TooltipContent>
          </Tooltip>

          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={() => router.push('/settings')}
                className={`p-2 rounded-lg transition-colors duration-150 ${isSettingsPage ? 'bg-accent text-foreground' : 'text-muted-foreground hover:bg-accent hover:text-foreground'
                  }`}
              >
                <Settings className="w-5 h-5" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              <p>Settings</p>
            </TooltipContent>
          </Tooltip>

          <Info isCollapsed={isCollapsed} />
        </div>
      </TooltipProvider>
    );
  };

  const renderItem = (item: SidebarItem, depth = 0) => {
    const isExpanded = expandedFolders.has(item.id);
    const paddingLeft = `${depth * 12 + 12}px`;
    const isActive = item.type === 'file' && currentMeeting?.id === item.id;
    const isMeetingItem = item.id.includes('-') && !item.id.startsWith('intro-call');

    if (isCollapsed) return null;

    return (
      <div key={item.id}>
        <div
          className={`relative flex items-center transition-all duration-150 group ${item.type === 'folder' && depth === 0
            ? 'p-3 text-sm font-semibold h-10 mx-3 mt-3 rounded-lg'
            : `px-3 py-2 my-0.5 rounded-md text-sm ${isActive ? 'bg-brand/10 text-foreground font-medium' : 'hover:bg-accent/60'
            } cursor-pointer`
            }`}
          style={item.type === 'folder' && depth === 0 ? {} : { paddingLeft }}
          onClick={() => {
            if (item.type === 'folder') {
              toggleFolder(item.id);
            } else {
              setCurrentMeeting({ id: item.id, title: item.title });
              const basePath = item.id.startsWith('intro-call') ? '/' : `/meeting-details?id=${item.id}`;
              router.push(basePath);
            }
          }}
        >
          {isActive && (
            <motion.span
              layoutId="sidebar-active-rail"
              className="absolute left-0 top-1.5 bottom-1.5 w-0.5 rounded-full bg-brand"
            />
          )}
          {item.type === 'folder' ? (
            <>
              {item.id === 'meetings' ? (
                <Calendar className="w-4 h-4 mr-2" />
              ) : item.id === 'notes' ? (
                <Calendar className="w-4 h-4 mr-2" />
              ) : null}
              <span className={depth === 0 ? "" : "font-medium"}>{item.title}</span>
              <div className="ml-auto">
                {isExpanded ? (
                  <ChevronDown className="w-4 h-4 text-muted-foreground" />
                ) : (
                  <ChevronRight className="w-4 h-4 text-muted-foreground" />
                )}
              </div>
            </>
          ) : (
            <div className="flex flex-col w-full">
              <div className="flex items-center w-full">
                {isMeetingItem ? (
                  <div className="flex-shrink-0 flex items-center justify-center w-6 h-6 rounded-full mr-2 bg-muted">
                    <File className="w-3.5 h-3.5 text-muted-foreground" />
                  </div>
                ) : (
                  <div className="flex-shrink-0 flex items-center justify-center w-6 h-6 rounded-full mr-2 bg-brand/15">
                    <Plus className="w-3.5 h-3.5 text-brand" />
                  </div>
                )}
                <span className="flex-1 break-words">{item.title}</span>
                {isMeetingItem && (
                  // Dimmed rather than hidden: the row actions used to be
                  // opacity-0 until hover, which made rename/delete impossible
                  // to find (and unreachable without a pointer).
                  <MeetingActionsMenu
                    meetingId={item.id}
                    title={item.title}
                    className="opacity-60 group-hover:opacity-100 focus-visible:opacity-100 data-[state=open]:opacity-100 transition-opacity duration-150"
                    onRenamed={(newTitle) => handleMeetingRenamed(item.id, newTitle)}
                    onTrashed={() => handleMeetingTrashed(item.id)}
                    onRestored={refetchMeetings}
                  />
                )}
              </div>
            </div>
          )}
        </div>
        {item.type === 'folder' && isExpanded && item.children && (
          <div className="ml-1">
            {item.children.map(child => renderItem(child, depth + 1))}
          </div>
        )}
      </div>
    );
  };

  return (
    <div className="fixed top-[var(--titlebar-height)] left-0 h-[calc(100vh-var(--titlebar-height))] z-40">
      {/* On small screens the expanded sidebar overlays the content instead of
          pushing it; this backdrop dims the content and collapses on click */}
      {!isCollapsed && (
        <div
          className="fixed inset-0 bg-overlay/40 md:hidden"
          onClick={toggleCollapse}
        />
      )}

      <div
        className={`h-full bg-card border-r border-border flex flex-col transition-all duration-300 ${isCollapsed ? 'w-16' : 'w-64'
          }`}
      >
        {/* Main content - scrollable area */}
        <div className="flex-1 flex flex-col min-h-0">
          {/* Fixed navigation items. The collapse toggle rides at the right
              edge of the Home row so it doesn't need a dedicated row, and the
              window titlebar carries the app branding. Search lives on the
              /meetings page now, so there's no sidebar search box. */}
          <div className="flex-shrink-0">
            {!isCollapsed && (
              <>
                <div className="flex items-center gap-1 mx-3 mt-3">
                  <div
                    onClick={() => router.push('/')}
                    className={`flex-1 min-w-0 p-3 text-sm font-medium items-center h-10 flex rounded-lg cursor-pointer transition-colors ${pathname === '/' ? 'bg-accent text-foreground' : 'text-muted-foreground hover:text-foreground hover:bg-accent'
                      }`}
                  >
                    <Home className="w-4 h-4 mr-2" />
                    <span>Home</span>
                  </div>
                  <button
                    onClick={toggleCollapse}
                    aria-label="Collapse sidebar"
                    title="Collapse sidebar"
                    className="shrink-0 p-2 rounded-lg text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
                  >
                    <PanelLeftClose className="w-5 h-5" />
                  </button>
                </div>
                <div
                  onClick={() => router.push('/meetings')}
                  className={`p-3 text-sm font-medium items-center h-10 flex mx-3 mt-1 rounded-lg cursor-pointer transition-colors ${pathname === '/meetings' ? 'bg-accent text-foreground' : 'text-muted-foreground hover:text-foreground hover:bg-accent'
                    }`}
                >
                  <List className="w-4 h-4 mr-2" />
                  <span>Meetings</span>
                </div>
                <div
                  onClick={() => router.push('/projects')}
                  className={`p-3 text-sm font-medium items-center h-10 flex mx-3 mt-1 rounded-lg cursor-pointer transition-colors ${pathname === '/projects' || pathname === '/project-details' ? 'bg-accent text-foreground' : 'text-muted-foreground hover:text-foreground hover:bg-accent'
                    }`}
                >
                  <FolderKanban className="w-4 h-4 mr-2" />
                  <span>Projects</span>
                </div>
              </>
            )}
          </div>

          {/* Content area */}
          <div className="flex-1 flex flex-col min-h-0">
            {renderCollapsedIcons()}
            {/* Recent meetings. The full, date-grouped, searchable list now
                lives on the /meetings page — the sidebar keeps only a short
                "Recent" shortlist with a "View all" link. */}
            {!isCollapsed && (() => {
              const meetingsFolder = sidebarItems.find(
                item => item.type === 'folder' && item.id === 'meetings'
              );
              // Real meeting items only (drop the "+ New Call" intro item).
              const children = (meetingsFolder?.children ?? []).filter(
                child => !child.id.startsWith('intro-call')
              );

              // Newest ~5 meetings + a "View all" link to /meetings.
              const recent = [...children]
                .sort((a, b) => {
                  const ta = a.createdAt ? new Date(a.createdAt).getTime() : 0;
                  const tb = b.createdAt ? new Date(b.createdAt).getTime() : 0;
                  return tb - ta;
                })
                .slice(0, 5);

              return (
                <>
                  <div className="flex-shrink-0 flex items-center justify-between pl-3 pr-4 mt-3 h-10">
                    <div className="flex items-center text-sm font-medium">
                      <NotebookPen className="w-4 h-4 mr-2 text-muted-foreground" />
                      <span className="text-muted-foreground">Recent</span>
                    </div>
                    <button
                      onClick={() => router.push('/meetings')}
                      className="text-xs font-medium text-brand hover:underline"
                    >
                      View all →
                    </button>
                  </div>
                  <div className="flex-1 overflow-y-auto custom-scrollbar min-h-0">
                    <div className="mx-3">
                      {recent.length === 0 ? (
                        <div className="px-2 py-3 text-xs text-muted-foreground">
                          No meetings yet
                        </div>
                      ) : (
                        recent.map(child => renderItem(child, 1))
                      )}
                    </div>
                  </div>
                </>
              );
            })()}
          </div>
        </div>

        {/* Footer */}
        {!isCollapsed && (

          <div className="flex-shrink-0 p-2 border-t border-border">
            <button
              onClick={handleRecordingToggle}
              disabled={isRecording}
              className={`w-full flex items-center justify-center px-3 py-2 text-sm font-medium rounded-lg transition-colors ${isRecording
                ? 'bg-destructive/15 text-destructive cursor-not-allowed shadow-glow-destructive'
                : 'bg-destructive text-destructive-foreground hover:bg-destructive/90'}`}
            >
              {isRecording ? (
                <>
                  <Square className="w-4 h-4 mr-2" />
                  <span>Recording in progress...</span>
                </>
              ) : (
                <>
                  <Mic className="w-4 h-4 mr-2" />
                  <span>Start Recording</span>
                </>
              )}
            </button>

            {betaFeatures.importAndRetranscribe && (
              <button
                onClick={() => openImportDialog()}
                className="w-full flex items-center justify-center px-3 py-2 mt-1 text-sm font-medium text-brand bg-brand/10 hover:bg-brand/20 rounded-lg transition-colors"
              >
                <Upload className="w-4 h-4 mr-2" />
                <span>Import Audio</span>
              </button>
            )}

            <button
              onClick={() => router.push('/trash')}
              className={`w-full flex items-center justify-center px-3 py-1.5 mt-1 text-sm font-medium rounded-lg transition-colors ${pathname === '/trash' ? 'bg-accent text-foreground' : 'text-muted-foreground hover:text-foreground hover:bg-accent'
                }`}
            >
              <Trash2 className="w-4 h-4 mr-2" />
              <span>Trash</span>
            </button>

            <button
              onClick={() => router.push('/settings')}
              className={`w-full flex items-center justify-center px-3 py-1.5 mt-1 mb-1 text-sm font-medium rounded-lg transition-colors ${pathname === '/settings' ? 'bg-accent text-foreground' : 'text-muted-foreground hover:text-foreground hover:bg-accent'
                }`}
            >
              <Settings className="w-4 h-4 mr-2" />
              <span>Settings</span>
            </button>
            <Info isCollapsed={isCollapsed} />
            <GlobalEgressIndicator />
            {appVersion && (
              <div className="w-full flex items-center justify-center px-3 py-1 text-xs text-muted-foreground/70 font-mono">
                v{appVersion}
              </div>
            )}
          </div>
        )}
      </div>

    </div>
  );
};

export default Sidebar;
