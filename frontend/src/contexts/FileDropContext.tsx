'use client';

import { createContext, useContext, useEffect, MutableRefObject, ReactNode } from 'react';

/**
 * A page's claim on OS file drops. While a claim is registered, the global
 * drag-drop listeners in layout.tsx route drops here instead of the default
 * (beta-gated) audio-import flow.
 */
export interface FileDropClaim {
  /** Return true when the drop was handled; false falls through to the default handling. */
  onDrop: (paths: string[]) => boolean;
  overlay: { title: string; subtitle?: string };
}

const FileDropContext = createContext<MutableRefObject<FileDropClaim | null> | null>(null);

interface FileDropProviderProps {
  /** Owned by layout.tsx so its drag-drop listeners can read the active claim. */
  claimRef: MutableRefObject<FileDropClaim | null>;
  children: ReactNode;
}

export function FileDropProvider({ claimRef, children }: FileDropProviderProps) {
  return <FileDropContext.Provider value={claimRef}>{children}</FileDropContext.Provider>;
}

/**
 * Register a drop claim for the lifetime of the calling component. Pass a
 * memoized claim (useMemo) — a new object each render would re-register.
 * Pass null to render without claiming (e.g. while data is loading).
 */
export function useFileDropTarget(claim: FileDropClaim | null) {
  const claimRef = useContext(FileDropContext);
  if (!claimRef) throw new Error('useFileDropTarget must be used within FileDropProvider');

  useEffect(() => {
    if (!claim) return;
    claimRef.current = claim;
    return () => {
      if (claimRef.current === claim) {
        claimRef.current = null;
      }
    };
  }, [claimRef, claim]);
}
