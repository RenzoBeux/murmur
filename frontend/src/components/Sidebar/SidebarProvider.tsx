'use client';

import React, { createContext, useContext, useState, useEffect } from 'react';
import { usePathname, useRouter } from 'next/navigation';
import { invoke } from '@tauri-apps/api/core';
import { useRecordingState } from '@/contexts/RecordingStateContext';


interface SidebarItem {
  id: string;
  title: string;
  type: 'folder' | 'file';
  children?: SidebarItem[];
  createdAt?: string;
}

export interface CurrentMeeting {
  id: string;
  title: string;
  createdAt?: string;
  updatedAt?: string;
}

// Search result type for transcript search
interface TranscriptSearchResult {
  id: string;
  title: string;
  matchContext: string;
  timestamp: string;
};

interface SidebarContextType {
  currentMeeting: CurrentMeeting | null;
  setCurrentMeeting: (meeting: CurrentMeeting | null) => void;
  sidebarItems: SidebarItem[];
  isCollapsed: boolean;
  toggleCollapse: () => void;
  meetings: CurrentMeeting[];
  // Raw useState dispatch: accepts a value OR a functional updater. The updater
  // form is required for correctness when mutating from async callbacks (e.g.
  // overlapping deletes) so each update builds on the latest state, not a stale
  // render-time snapshot.
  setMeetings: React.Dispatch<React.SetStateAction<CurrentMeeting[]>>;
  isMeetingActive: boolean;
  setIsMeetingActive: (active: boolean) => void;
  handleRecordingToggle: () => void;
  searchTranscripts: (query: string) => Promise<void>;
  searchResults: TranscriptSearchResult[];
  isSearching: boolean;
  // Summary polling management
  startSummaryPolling: (meetingId: string, processId: string, onUpdate: (result: any) => void) => void;
  stopSummaryPolling: (meetingId: string) => void;
  // Refetch meetings from backend
  refetchMeetings: () => Promise<void>;

}

const SidebarContext = createContext<SidebarContextType | null>(null);

export const useSidebar = () => {
  const context = useContext(SidebarContext);
  if (!context) {
    throw new Error('useSidebar must be used within a SidebarProvider');
  }
  return context;
};

export function SidebarProvider({ children }: { children: React.ReactNode }) {
  const [currentMeeting, setCurrentMeeting] = useState<CurrentMeeting | null>({ id: 'intro-call', title: '+ New Call' });
  const [isCollapsed, setIsCollapsed] = useState(true);
  const [meetings, setMeetings] = useState<CurrentMeeting[]>([]);
  const [sidebarItems, setSidebarItems] = useState<SidebarItem[]>([]);
  const [isMeetingActive, setIsMeetingActive] = useState(false);
  const [searchResults, setSearchResults] = useState<any[]>([]);
  const [isSearching, setIsSearching] = useState(false);
  // Active summary poll intervals, keyed by meetingId. A ref (not state) so that
  // starting/stopping one meeting's poll never re-runs effects that would tear
  // down another meeting's poll (the old state-Map cleanup killed all polls).
  const activeSummaryPolls = React.useRef<Map<string, NodeJS.Timeout>>(new Map());

  // Use recording state from RecordingStateContext (single source of truth)
  const { isRecording } = useRecordingState();

  const pathname = usePathname();
  const router = useRouter();

  // Extract fetchMeetings as a reusable function
  const fetchMeetings = React.useCallback(async () => {
    try {
      const meetings = await invoke('api_get_meetings') as Array<{ id: string, title: string, created_at: string, updated_at: string }>;
      const transformedMeetings = meetings.map((meeting: any) => ({
        id: meeting.id,
        title: meeting.title,
        createdAt: meeting.created_at,
        updatedAt: meeting.updated_at
      }));
      setMeetings(transformedMeetings);
    } catch (error) {
      console.error('Error fetching meetings:', error);
      setMeetings([]);
    }
  }, []);

  useEffect(() => {
    fetchMeetings();
  }, [fetchMeetings]);

  const baseItems: SidebarItem[] = [
    {
      id: 'meetings',
      title: 'Meeting Notes',
      type: 'folder' as const,
      children: [
        ...meetings.map(meeting => ({ id: meeting.id, title: meeting.title, type: 'file' as const, createdAt: meeting.createdAt }))
      ]
    },
  ];


  const toggleCollapse = () => {
    setIsCollapsed(!isCollapsed);
  };

  // Update current meeting when on home page
  useEffect(() => {
    if (pathname === '/') {
      setCurrentMeeting({ id: 'intro-call', title: '+ New Call' });
    }
    setSidebarItems(baseItems);
  }, [pathname]);

  // Update sidebar items when meetings change
  useEffect(() => {
    setSidebarItems(baseItems);
  }, [meetings]);

  // Function to handle recording toggle from sidebar
  const handleRecordingToggle = () => {
    if (!isRecording) {
      // Check if already on home page
      if (pathname === '/') {
        // Already on home - trigger recording directly via custom event
        console.log('Triggering recording from sidebar (already on home page)');
        window.dispatchEvent(new CustomEvent('start-recording-from-sidebar'));
      } else {
        // Not on home - navigate and use auto-start mechanism
        console.log('Navigating to home page with auto-start flag');
        sessionStorage.setItem('autoStartRecording', 'true');
        router.push('/');
      }
    }
    // The actual recording start/stop is handled in the Home component
  };

  // Monotonic guard so a slow earlier search response can't clobber a newer one.
  // invoke() isn't abortable, so we discard stale results rather than cancelling them.
  const searchSeqRef = React.useRef(0);

  // Function to search through meeting transcripts
  const searchTranscripts = async (query: string) => {
    // Bump for every call (including the empty-query reset) so any inflight response
    // for a prior query is discarded.
    const seq = ++searchSeqRef.current;

    if (!query.trim()) {
      setSearchResults([]);
      setIsSearching(false);
      return;
    }

    try {
      setIsSearching(true);

      const results = await invoke('api_search_transcripts', { query }) as TranscriptSearchResult[];
      if (seq !== searchSeqRef.current) return; // superseded by a newer search
      setSearchResults(results);
    } catch (error) {
      if (seq !== searchSeqRef.current) return;
      console.error('Error searching transcripts:', error);
      setSearchResults([]);
    } finally {
      // Only clear the spinner if this is still the latest request.
      if (seq === searchSeqRef.current) setIsSearching(false);
    }
  };

  // Summary polling management
  const startSummaryPolling = React.useCallback((
    meetingId: string,
    processId: string,
    onUpdate: (result: any) => void
  ) => {
    // Stop existing poll for this meeting if any
    const existing = activeSummaryPolls.current.get(meetingId);
    if (existing) {
      clearInterval(existing);
    }

    console.log(`📊 Starting polling for meeting ${meetingId}, process ${processId}`);

    let pollCount = 0;
    // ~60 minutes at 5s intervals. The old 16.5-min cap fired an error while a long
    // local model run was legitimately still going, inviting a duplicate regeneration
    // that corrupts the cancellation registry. A process genuinely orphaned by a quit is
    // reset to 'failed' by the startup sweep (reset_orphaned_processes) and caught by the
    // terminal-status stop below, so this cap only bounds truly pathological runs.
    const MAX_POLLS = 720;

    const pollInterval = setInterval(async () => {
      pollCount++;

      // Absolute safety cap.
      if (pollCount >= MAX_POLLS) {
        console.warn(`⏱️ Polling cap reached for ${meetingId} after ${MAX_POLLS} iterations`);
        clearInterval(pollInterval);
        activeSummaryPolls.current.delete(meetingId);
        onUpdate({
          status: 'error',
          error: 'Summary is taking unusually long (over an hour). It may still finish in the background — reopen the meeting to check, or try again.'
        });
        return;
      }
      try {
        const result = await invoke('api_get_summary', {
          meetingId: meetingId,
        }) as any;

        console.log(`📊 Polling update for ${meetingId}:`, result.status);

        // Call the update callback with result
        onUpdate(result);

        // Stop polling if completed, error, failed, cancelled, or idle (after initial processing)
        if (result.status === 'completed' || result.status === 'error' || result.status === 'failed' || result.status === 'cancelled') {
          console.log(`Polling completed for ${meetingId}, status: ${result.status}`);
          clearInterval(pollInterval);
          activeSummaryPolls.current.delete(meetingId);
        } else if (result.status === 'idle' && pollCount > 1) {
          // If we get 'idle' after polling started, process completed/disappeared
          console.log(`Process completed or not found for ${meetingId}, stopping poll`);
          clearInterval(pollInterval);
          activeSummaryPolls.current.delete(meetingId);
        }
      } catch (error) {
        console.error(`Polling error for ${meetingId}:`, error);
        // Report error to callback
        onUpdate({
          status: 'error',
          error: error instanceof Error ? error.message : 'Unknown error'
        });
        clearInterval(pollInterval);
        activeSummaryPolls.current.delete(meetingId);
      }
    }, 5000); // Poll every 5 seconds

    activeSummaryPolls.current.set(meetingId, pollInterval);
  }, []);

  const stopSummaryPolling = React.useCallback((meetingId: string) => {
    const pollInterval = activeSummaryPolls.current.get(meetingId);
    if (pollInterval) {
      console.log(`⏹️ Stopping polling for meeting ${meetingId}`);
      clearInterval(pollInterval);
      activeSummaryPolls.current.delete(meetingId);
    }
  }, []);

  // Clear all polling intervals on unmount only (run-once cleanup — must not
  // depend on the poll map, or starting a new poll would tear down the others).
  useEffect(() => {
    const polls = activeSummaryPolls.current;
    return () => {
      console.log('🧹 Cleaning up all summary polling intervals');
      polls.forEach(interval => clearInterval(interval));
    };
  }, []);



  return (
    <SidebarContext.Provider value={{
      currentMeeting,
      setCurrentMeeting,
      sidebarItems,
      isCollapsed,
      toggleCollapse,
      meetings,
      setMeetings,
      isMeetingActive,
      setIsMeetingActive,
      handleRecordingToggle,
      searchTranscripts,
      searchResults,
      isSearching,
      startSummaryPolling,
      stopSummaryPolling,
      refetchMeetings: fetchMeetings,

    }}>
      {children}
    </SidebarContext.Provider>
  );
}
