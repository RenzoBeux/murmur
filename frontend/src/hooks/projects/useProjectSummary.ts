import { useCallback, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import {
  ProjectSummary,
  cancelProjectSummary,
  generateProjectSummary,
  getProjectSummary,
  isGenerating,
} from '@/lib/projectSummaryApi';

/** Matches SidebarProvider's summary poller, and the backend's stage cadence. */
const POLL_INTERVAL_MS = 5000;
/** ~1 hour, same bound the meeting summary poller uses. */
const MAX_POLLS = 720;

interface UseProjectSummaryProps {
  projectId: string;
  provider: string;
  model: string;
}

/**
 * The project brief, and the lifecycle of generating one.
 *
 * Owns a local poller rather than extending `SidebarProvider.startSummaryPolling`,
 * which hardcodes `api_get_summary` and keys its map by meeting id. A local one
 * is ~40 lines and dies with the hook.
 *
 * **Mount this above the tab strip, not inside the Summary tab.** Radix unmounts
 * inactive `TabsContent`, so a poller living in the panel would be torn down the
 * moment the user glanced at another tab and a running generation would go
 * unwatched.
 */
export function useProjectSummary({ projectId, provider, model }: UseProjectSummaryProps) {
  const [summary, setSummary] = useState<ProjectSummary | null>(null);
  const [isLoading, setIsLoading] = useState(true);

  const projectIdRef = useRef(projectId);
  projectIdRef.current = projectId;
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const pollCountRef = useRef(0);

  const stopPolling = useCallback(() => {
    if (pollRef.current) {
      clearInterval(pollRef.current);
      pollRef.current = null;
    }
    pollCountRef.current = 0;
  }, []);

  /**
   * Re-read the row. Never nulls `summary` — blanking on every poll would make
   * the brief flicker away and collapse the reader's scroll position.
   */
  const refresh = useCallback(async (): Promise<ProjectSummary | null> => {
    if (!projectId) return null;
    try {
      const next = await getProjectSummary(projectId);
      if (projectIdRef.current !== projectId) return null;
      setSummary(next);
      return next;
    } catch (err) {
      console.error('Failed to load project brief:', err);
      return null;
    } finally {
      if (projectIdRef.current === projectId) setIsLoading(false);
    }
  }, [projectId]);

  const startPolling = useCallback(() => {
    stopPolling();
    pollRef.current = setInterval(async () => {
      pollCountRef.current += 1;
      if (pollCountRef.current > MAX_POLLS) {
        stopPolling();
        toast.error('Project brief is taking over an hour', {
          description: 'Reopen the project to check on it.',
        });
        return;
      }
      const next = await refresh();
      if (next && !isGenerating(next)) stopPolling();
    }, POLL_INTERVAL_MS);
  }, [refresh, stopPolling]);

  // On mount (and whenever the project changes) read once, and re-attach to a
  // run already in flight. This is what makes navigating away and back work.
  useEffect(() => {
    setIsLoading(true);
    setSummary(null);
    let cancelled = false;
    void refresh().then((next) => {
      if (!cancelled && next && isGenerating(next)) startPolling();
    });
    return () => {
      cancelled = true;
      stopPolling();
    };
  }, [refresh, startPolling, stopPolling]);

  // Progress events make a multi-minute job visibly move between polls. The row
  // stays the source of truth; a dropped event just means the next poll reports it.
  useEffect(() => {
    const unlisten = listen<{ project_id: string; stage: string; current: number; total: number }>(
      'project-summary-progress',
      (event) => {
        if (event.payload.project_id !== projectIdRef.current) return;
        setSummary((prev) =>
          prev
            ? {
                ...prev,
                progress: {
                  stage: event.payload.stage,
                  current: event.payload.current,
                  total: event.payload.total,
                },
              }
            : prev,
        );
      },
    );
    return () => {
      void unlisten.then((off) => off());
    };
  }, []);

  const generate = useCallback(async () => {
    if (!projectId) return;
    if (!provider || !model) {
      toast.error('Pick a model before generating a project brief.');
      return;
    }
    // Flip immediately so the panel responds to the click, then let the poll
    // correct it. The backend refuses a second concurrent run regardless.
    setSummary((prev) =>
      prev ? { ...prev, status: 'PENDING', error: null } : prev,
    );
    try {
      const result = await generateProjectSummary(projectId, provider, model);
      if (result.alreadyRunning) {
        toast.info('A brief is already being generated for this project.');
      }
      startPolling();
      await refresh();
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      console.error('Failed to start project brief:', err);
      toast.error('Failed to start project brief', { description: msg });
      await refresh();
    }
  }, [projectId, provider, model, refresh, startPolling]);

  const cancel = useCallback(async () => {
    if (!projectId) return;
    try {
      await cancelProjectSummary(projectId);
    } catch (err) {
      console.error('Failed to cancel project brief:', err);
    } finally {
      stopPolling();
      await refresh();
    }
  }, [projectId, refresh, stopPolling]);

  return {
    summary,
    isLoading,
    isGenerating: isGenerating(summary),
    generate,
    cancel,
    refresh,
  };
}
