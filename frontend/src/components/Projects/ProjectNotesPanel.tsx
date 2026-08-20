"use client";

import { useCallback, useEffect, useRef, useState } from 'react';
import { AlertTriangle, Check, Loader2 } from 'lucide-react';
import { toast } from 'sonner';

import { Textarea } from '@/components/ui/textarea';
import { setProjectContextNotes } from '@/lib/projectsApi';

/** Idle time after the last keystroke before a save fires. */
const AUTOSAVE_DELAY_MS = 1000;

type SaveState = 'idle' | 'dirty' | 'saving' | 'saved' | 'error';

interface ProjectNotesPanelProps {
  projectId: string;
  /** The stored notes. null while the project is still loading. */
  notes: string | null;
  /** Called after a successful save so the page's copy stays in step. */
  onSaved: (notes: string) => void;
}

const PLACEHOLDER = `Anything the AI should know that the recordings don't say. For example:

Acme is the client. Sofía is their PM — transcripts usually write "Sofi".
"Titan" is our name for the v2 rewrite.
Budget was signed off in February, so cost questions are settled.
Prefer short answers.`;

/**
 * Free-form project context, written by the user and read by the project chat
 * and the project brief.
 *
 * Autosaves rather than offering a Save button. These notes get written once and
 * left alone for weeks, so the failure mode of an explicit save — typing a
 * paragraph, switching tabs, losing it — is both likely and unrecoverable. It is
 * also specifically dangerous here: Radix unmounts the inactive tab, so a
 * dirty-state guard would have to fight the tab strip to work at all.
 */
export function ProjectNotesPanel({ projectId, notes, onSaved }: ProjectNotesPanelProps) {
  const [value, setValue] = useState(notes ?? '');
  const [saveState, setSaveState] = useState<SaveState>('idle');

  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const projectIdRef = useRef(projectId);
  projectIdRef.current = projectId;
  // What is on disk, so a save is skipped when nothing actually changed.
  const savedValueRef = useRef(notes ?? '');
  // The latest text, readable from the timer without re-arming it on every key.
  const valueRef = useRef(value);
  valueRef.current = value;

  // Reseed when the project changes, or when the stored notes first arrive.
  // Guarded on the saved baseline rather than on `value`, so a poll or a
  // refetch landing mid-sentence cannot overwrite what is being typed.
  useEffect(() => {
    const incoming = notes ?? '';
    if (incoming !== savedValueRef.current) {
      savedValueRef.current = incoming;
      setValue(incoming);
      setSaveState('idle');
    }
  }, [projectId, notes]);

  const save = useCallback(
    async (text: string) => {
      const targetProjectId = projectIdRef.current;
      if (text === savedValueRef.current) {
        setSaveState('idle');
        return;
      }
      setSaveState('saving');
      try {
        await setProjectContextNotes(targetProjectId, text.trim() === '' ? null : text);
        if (projectIdRef.current !== targetProjectId) return;
        savedValueRef.current = text;
        setSaveState('saved');
        onSaved(text);
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error('Failed to save project notes:', err);
        if (projectIdRef.current !== targetProjectId) return;
        setSaveState('error');
        toast.error('Failed to save notes', { description: msg });
      }
    },
    [onSaved],
  );

  const handleChange = (next: string) => {
    setValue(next);
    setSaveState('dirty');
    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = setTimeout(() => void save(next), AUTOSAVE_DELAY_MS);
  };

  // Flush a pending edit rather than dropping it when the tab is switched away
  // (Radix unmounts it) or the window closes.
  useEffect(() => {
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
      if (valueRef.current !== savedValueRef.current) {
        void setProjectContextNotes(
          projectIdRef.current,
          valueRef.current.trim() === '' ? null : valueRef.current,
        ).catch((err) => console.error('Failed to flush project notes:', err));
      }
    };
  }, []);

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between gap-3 border-b border-border px-4 py-2">
        <p className="text-xs text-muted-foreground">
          Background for the AI. Read by this project’s chat and brief on every answer — never
          quoted as something said in a meeting.
        </p>
        <SaveStatus state={saveState} />
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto">
        <div className="mx-auto h-full max-w-3xl px-6 py-5">
          <Textarea
            value={value}
            onChange={(e) => handleChange(e.target.value)}
            onBlur={() => {
              if (timerRef.current) clearTimeout(timerRef.current);
              void save(valueRef.current);
            }}
            placeholder={PLACEHOLDER}
            spellCheck
            className="h-full min-h-[320px] resize-none border-border bg-transparent text-sm leading-6 focus-visible:ring-1"
          />
        </div>
      </div>
    </div>
  );
}

function SaveStatus({ state }: { state: SaveState }) {
  if (state === 'saving') {
    return (
      <span className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
        <Loader2 className="h-3 w-3 animate-spin" />
        Saving…
      </span>
    );
  }
  if (state === 'saved') {
    return (
      <span className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
        <Check className="h-3 w-3" />
        Saved
      </span>
    );
  }
  if (state === 'error') {
    return (
      <span className="flex shrink-0 items-center gap-1.5 text-xs text-amber-600 dark:text-amber-500">
        <AlertTriangle className="h-3 w-3" />
        Not saved
      </span>
    );
  }
  if (state === 'dirty') {
    return <span className="shrink-0 text-xs text-muted-foreground">Unsaved changes…</span>;
  }
  return null;
}
