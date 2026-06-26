'use client';

import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { Trash2, Combine, Users, X } from 'lucide-react';
import { SpeakerPicker } from './SpeakerPicker';
import { formatSpeaker } from '@/lib/speakerLabel';

interface TranscriptEditorToolbarProps {
  selectionCount: number;
  canMerge: boolean;
  mergeBlockedReason?: string;
  /**
   * If the user attempts a merge, the panel pre-validates and reports the
   * distinct speakers among the selection. When `length > 1` the toolbar
   * shows an inline picker to resolve the conflict before committing.
   */
  getMergeSpeakers?: () => string[];
  knownSpeakers: string[];
  onMerge?: (speakerOverride?: string | null) => void;
  onReassignSpeaker?: (speaker: string | null) => void;
  onDelete?: () => void;
  onClear: () => void;
}

export function TranscriptEditorToolbar({
  selectionCount,
  canMerge,
  mergeBlockedReason,
  getMergeSpeakers,
  knownSpeakers,
  onMerge,
  onReassignSpeaker,
  onDelete,
  onClear,
}: TranscriptEditorToolbarProps) {
  const hasSelection = selectionCount > 0;
  const [pendingMergeSpeakers, setPendingMergeSpeakers] = useState<string[] | null>(null);

  const handleMergeClick = () => {
    if (!onMerge) return;
    const speakers = getMergeSpeakers?.() ?? [];
    if (speakers.length > 1) {
      setPendingMergeSpeakers(speakers);
      return;
    }
    onMerge();
  };

  return (
    <div className="sticky top-0 z-20 bg-amber-50 border-b border-amber-200 px-3 py-2 flex flex-col gap-2">
      <div className="flex items-center gap-2 flex-wrap">
      <span className="text-xs font-medium text-amber-900 mr-1">
        {hasSelection ? `${selectionCount} selected` : 'Edit mode — click rows to select'}
      </span>

      <Button
        size="sm"
        variant="outline"
        disabled={!canMerge}
        onClick={handleMergeClick}
        title={canMerge ? 'Merge selected segments' : (mergeBlockedReason ?? 'Select ≥2 contiguous segments to merge')}
      >
        <Combine size={14} />
        <span className="hidden @lg:inline ml-1">Merge</span>
      </Button>

      {hasSelection && onReassignSpeaker ? (
        <SpeakerPicker
          knownSpeakers={knownSpeakers}
          onPick={onReassignSpeaker}
          trigger={
            <Button size="sm" variant="outline" title="Reassign speaker on selected segments">
              <Users size={14} />
              <span className="hidden @lg:inline ml-1">Speaker</span>
            </Button>
          }
        />
      ) : (
        <Button
          size="sm"
          variant="outline"
          disabled
          title="Select segments first"
        >
          <Users size={14} />
          <span className="hidden @lg:inline ml-1">Speaker</span>
        </Button>
      )}

      <Button
        size="sm"
        variant="outline"
        disabled={!hasSelection}
        onClick={onDelete}
        title={hasSelection ? 'Delete selected segments' : 'Select segments first'}
      >
        <Trash2 size={14} />
        <span className="hidden @lg:inline ml-1">Delete</span>
      </Button>

      {hasSelection && (
        <button
          type="button"
          onClick={onClear}
          className="ml-auto inline-flex items-center text-xs text-amber-900 hover:underline"
        >
          <X size={12} className="mr-0.5" />
          Clear
        </button>
      )}
      </div>

      {pendingMergeSpeakers && (
        <div className="bg-white border border-amber-300 rounded p-2 flex items-center gap-2 flex-wrap">
          <span className="text-xs text-gray-700">
            Different speakers — pick one for the merged segment:
          </span>
          {pendingMergeSpeakers.map((s) => {
            const label = formatSpeaker(s);
            return (
              <button
                key={s}
                type="button"
                onClick={() => {
                  onMerge?.(s);
                  setPendingMergeSpeakers(null);
                }}
                className={`text-xs px-2 py-1 rounded hover:ring-2 hover:ring-amber-300 ${label?.className ?? 'bg-gray-100'}`}
              >
                {label?.label ?? s}
              </button>
            );
          })}
          <button
            type="button"
            onClick={() => {
              onMerge?.(null);
              setPendingMergeSpeakers(null);
            }}
            className="text-xs px-2 py-1 rounded bg-gray-100 text-gray-600 hover:bg-gray-200"
          >
            (clear)
          </button>
          <button
            type="button"
            onClick={() => setPendingMergeSpeakers(null)}
            className="text-xs text-gray-500 hover:underline ml-auto"
          >
            Cancel
          </button>
        </div>
      )}
    </div>
  );
}
