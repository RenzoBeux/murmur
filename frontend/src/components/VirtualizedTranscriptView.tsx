'use client';

import { useCallback, useRef, useReducer, startTransition, useEffect, useState, memo } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useAutoScroll } from "@/hooks/useAutoScroll";
import { useTranscriptStreaming } from "@/hooks/useTranscriptStreaming";
import { ConfidenceIndicator } from "./ConfidenceIndicator";
import { Tooltip, TooltipContent, TooltipTrigger } from "./ui/tooltip";
import { RecordingStatusBar } from "./RecordingStatusBar";
import { motion, AnimatePresence } from "framer-motion";
import { TranscriptSegmentData } from "@/types";
import { formatSpeaker } from "@/lib/speakerLabel";
import { SpeakerPicker } from "./MeetingDetails/SpeakerPicker";
import { Logomark } from "./brand/Logomark";
import { ArrowDown } from "lucide-react";

/**
 * Edit-mode controls passed through to each row. When undefined the view
 * renders in read-only mode and behaves identically to before — the live
 * recording path relies on this.
 */
export interface VirtualizedTranscriptEditMode {
    selectedIds: Set<string>;
    editingId: string | null;
    knownSpeakers: string[];
    onToggleSelect: (id: string, withShift: boolean) => void;
    onStartEdit?: (id: string) => void;
    onCommitEdit?: (id: string, newText: string) => void;
    onCancelEdit?: () => void;
    onReassignRowSpeaker?: (id: string, speaker: string | null) => void;
    onSplit?: (id: string, charOffset: number, currentText: string) => void;
}

export interface VirtualizedTranscriptViewProps {
    /** Transcript segments to display */
    segments: TranscriptSegmentData[];
    /** Whether recording is in progress */
    isRecording?: boolean;
    /** Whether recording is paused */
    isPaused?: boolean;
    /** Whether processing/finalizing transcription */
    isProcessing?: boolean;
    /** Whether stopping */
    isStopping?: boolean;
    /** Enable streaming effect for latest segment */
    enableStreaming?: boolean;
    /** Show confidence indicators */
    showConfidence?: boolean;
    /** Completely disable auto-scroll behavior (for meeting details page) */
    disableAutoScroll?: boolean;
    /** Editor controls; when undefined the view is read-only. */
    editMode?: VirtualizedTranscriptEditMode;

    // Pagination props (infinite scroll)
    hasMore?: boolean;
    isLoadingMore?: boolean;
    totalCount?: number;
    loadedCount?: number;
    onLoadMore?: () => void;
}

// Threshold for enabling virtualization (below this, use simple rendering)
const VIRTUALIZATION_THRESHOLD = 10;

// Helper function to format seconds as recording-relative time [MM:SS]
function formatRecordingTime(seconds: number | undefined): string {
    if (seconds === undefined) return '[--:--]';

    const totalSeconds = Math.floor(seconds);
    const minutes = Math.floor(totalSeconds / 60);
    const secs = totalSeconds % 60;

    return `[${minutes.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}]`;
}

// Helper function to remove filler words and repetitions
function cleanStopWords(text: string): string {
    const stopWords = ['uh', 'um', 'er', 'ah', 'hmm', 'hm', 'eh', 'oh'];

    let cleanedText = text;
    stopWords.forEach(word => {
        const pattern = new RegExp(`\\b${word}\\b[,\\s]*`, 'gi');
        cleanedText = cleanedText.replace(pattern, ' ');
    });

    return cleanedText.replace(/\s+/g, ' ').trim();
}

// Memoized transcript segment component.
// Per-row edit booleans (isSelected, isEditing) are passed in directly rather
// than deriving from a Set in the row, so memo() can short-circuit cleanly.
const TranscriptSegment = memo(function TranscriptSegment({
    id,
    timestamp,
    text,
    confidence,
    speaker,
    isStreaming,
    showConfidence,
    editable,
    isSelected,
    isEditing,
    knownSpeakers,
    onToggleSelect,
    onStartEdit,
    onCommitEdit,
    onCancelEdit,
    onReassignRowSpeaker,
    onSplit,
}: {
    id: string;
    timestamp: number;
    text: string;
    confidence?: number;
    speaker?: string;
    isStreaming: boolean;
    showConfidence: boolean;
    editable: boolean;
    isSelected: boolean;
    isEditing: boolean;
    knownSpeakers: string[];
    onToggleSelect?: (id: string, withShift: boolean) => void;
    onStartEdit?: (id: string) => void;
    onCommitEdit?: (id: string, newText: string) => void;
    onCancelEdit?: () => void;
    onReassignRowSpeaker?: (id: string, speaker: string | null) => void;
    onSplit?: (id: string, charOffset: number, currentText: string) => void;
}) {
    const displayText = editable
        ? text || '[Silence]'
        : cleanStopWords(text) || (text.trim() === '' ? '[Silence]' : text);
    const speakerLabel = formatSpeaker(speaker);

    return (
        <div
            id={`segment-${id}`}
            className={`mb-3 ${editable && isSelected ? 'bg-brand/10 rounded' : ''}`}
        >
            <div className="flex items-start gap-2">
                {editable && (
                    <input
                        type="checkbox"
                        className="mt-1.5 flex-shrink-0"
                        checked={isSelected}
                        onChange={() => {}}
                        onClick={(e) => {
                            e.stopPropagation();
                            onToggleSelect?.(id, e.shiftKey);
                        }}
                        aria-label={`Select segment at ${formatRecordingTime(timestamp)}`}
                    />
                )}
                <Tooltip>
                    <TooltipTrigger>
                        <span className="text-xs text-muted-foreground/80 font-mono tabular-nums mt-1 flex-shrink-0 min-w-[50px]">
                            {formatRecordingTime(timestamp)}
                        </span>
                    </TooltipTrigger>
                    <TooltipContent>
                        {confidence !== undefined && showConfidence && (
                            <ConfidenceIndicator confidence={confidence} showIndicator={showConfidence} />
                        )}
                    </TooltipContent>
                </Tooltip>
                {speakerLabel && (editable && onReassignRowSpeaker ? (
                    <SpeakerPicker
                        knownSpeakers={knownSpeakers}
                        currentSpeaker={speaker}
                        onPick={(value) => onReassignRowSpeaker(id, value)}
                        trigger={
                            <button
                                type="button"
                                className={`text-xs px-1.5 py-0.5 rounded mt-1 flex-shrink-0 hover:ring-2 hover:ring-ring/50 ${speakerLabel.className}`}
                                title="Click to change speaker"
                            >
                                {speakerLabel.label}
                            </button>
                        }
                    />
                ) : (
                    <span className={`text-xs px-1.5 py-0.5 rounded mt-1 flex-shrink-0 ${speakerLabel.className}`}>
                        {speakerLabel.label}
                    </span>
                ))}
                {!speakerLabel && editable && onReassignRowSpeaker && (
                    <SpeakerPicker
                        knownSpeakers={knownSpeakers}
                        currentSpeaker={undefined}
                        onPick={(value) => onReassignRowSpeaker(id, value)}
                        trigger={
                            <button
                                type="button"
                                className="text-xs px-1.5 py-0.5 rounded mt-1 flex-shrink-0 bg-muted text-muted-foreground hover:ring-2 hover:ring-ring/50"
                                title="Set speaker"
                            >
                                + Speaker
                            </button>
                        }
                    />
                )}
                <div className="flex-1">
                    {editable && isEditing && onCommitEdit ? (
                        <InlineTextEditor
                            initialText={text}
                            onCommit={(newText) => onCommitEdit(id, newText)}
                            onCancel={() => onCancelEdit?.()}
                            onSplit={onSplit ? (offset, current) => onSplit(id, offset, current) : undefined}
                        />
                    ) : isStreaming ? (
                        <div className="bg-card border border-border border-l-2 border-l-brand rounded-lg px-3 py-2">
                            <p className="text-base text-foreground leading-relaxed">{displayText}</p>
                        </div>
                    ) : (
                        <p
                            className={`text-base text-foreground/90 leading-relaxed ${editable ? 'cursor-text hover:bg-accent/50 rounded px-1 -mx-1' : ''}`}
                            onClick={() => {
                                if (editable && onStartEdit) onStartEdit(id);
                            }}
                        >
                            {displayText}
                        </p>
                    )}
                </div>
            </div>
        </div>
    );
});

function InlineTextEditor({
    initialText,
    onCommit,
    onCancel,
    onSplit,
}: {
    initialText: string;
    onCommit: (newText: string) => void;
    onCancel: () => void;
    onSplit?: (charOffset: number, currentText: string) => void;
}) {
    const [value, setValue] = useState(initialText);
    const textareaRef = useRef<HTMLTextAreaElement>(null);
    const skipCommitOnBlurRef = useRef(false);

    useEffect(() => {
        const el = textareaRef.current;
        if (!el) return;
        el.focus();
        const len = el.value.length;
        el.setSelectionRange(len, len);
        el.style.height = 'auto';
        el.style.height = `${el.scrollHeight}px`;
    }, []);

    const commit = useCallback(() => {
        if (value === initialText) {
            onCancel();
            return;
        }
        onCommit(value);
    }, [value, initialText, onCommit, onCancel]);

    const triggerSplit = useCallback(() => {
        const el = textareaRef.current;
        if (!el || !onSplit) return;
        const offset = el.selectionStart ?? 0;
        // Suppress the blur-commit that fires when split mutates the row away.
        skipCommitOnBlurRef.current = true;
        onSplit(offset, value);
    }, [onSplit, value]);

    return (
        <div className="flex items-start gap-1">
            <textarea
                ref={textareaRef}
                value={value}
                onChange={(e) => {
                    setValue(e.target.value);
                    const el = e.currentTarget;
                    el.style.height = 'auto';
                    el.style.height = `${el.scrollHeight}px`;
                }}
                onBlur={() => {
                    if (skipCommitOnBlurRef.current) {
                        skipCommitOnBlurRef.current = false;
                        return;
                    }
                    commit();
                }}
                onKeyDown={(e) => {
                    if (e.key === 'Escape') {
                        e.preventDefault();
                        onCancel();
                    } else if (e.key === 'Enter' && e.ctrlKey) {
                        e.preventDefault();
                        triggerSplit();
                    } else if (e.key === 'Enter' && !e.shiftKey) {
                        e.preventDefault();
                        commit();
                    }
                }}
                className="w-full text-base text-foreground leading-relaxed bg-background border border-brand/40 rounded px-2 py-1 focus:outline-none focus:ring-1 focus:ring-ring resize-none overflow-hidden"
                rows={1}
            />
            {onSplit && (
                <button
                    type="button"
                    onMouseDown={(e) => {
                        // Prevent blur from firing before we read the caret.
                        e.preventDefault();
                    }}
                    onClick={triggerSplit}
                    className="text-xs text-warning hover:text-warning/80 hover:underline mt-1 px-1"
                    title="Split at caret (Ctrl+Enter)"
                >
                    ✂ Split
                </button>
            )}
        </div>
    );
}

export const VirtualizedTranscriptView: React.FC<VirtualizedTranscriptViewProps> = ({
    segments,
    isRecording = false,
    isPaused = false,
    isProcessing = false,
    isStopping = false,
    enableStreaming = false,
    showConfidence = true,
    disableAutoScroll = false,
    editMode,
    hasMore = false,
    isLoadingMore = false,
    totalCount = 0,
    loadedCount = 0,
    onLoadMore,
}) => {
    const editable = !!editMode;
    // Create scroll ref first - shared between virtualizer and auto-scroll hook
    const scrollRef = useRef<HTMLDivElement>(null);
    // Ref for infinite scroll trigger element
    const loadMoreTriggerRef = useRef<HTMLDivElement>(null);

    // Force re-render without flushSync (avoids React warning)
    const [, rerender] = useReducer((x: number) => x + 1, 0);

    // Setup virtualizer for efficient rendering of large lists
    const virtualizer = useVirtualizer({
        count: segments.length,
        getScrollElement: () => scrollRef.current,
        // Edit mode adds a checkbox row + interactive controls, so rows are
        // slightly taller. The virtualizer self-corrects via measureElement,
        // but starting closer avoids initial jitter.
        estimateSize: () => (editable ? 80 : 60),
        overscan: 10, // Render extra items above/below viewport
        onChange: () => {
            startTransition(() => {
                rerender();
            });
        },
    });

    // Custom hook for auto-scrolling (supports both virtualized and non-virtualized)
    const { scrollToBottom, isAtBottom } = useAutoScroll({
        scrollRef,
        segments,
        isRecording,
        isPaused,
        virtualizer,
        virtualizationThreshold: VIRTUALIZATION_THRESHOLD,
        disableAutoScroll,
    });

    // Streaming text effect hook (typewriter animation for new transcripts)
    const { streamingSegmentId, getDisplayText } = useTranscriptStreaming(
        segments,
        isRecording,
        enableStreaming
    );

    // Infinite scroll: IntersectionObserver to trigger loading more
    useEffect(() => {
        if (!onLoadMore || !hasMore || isLoadingMore || isRecording || segments.length === 0) {
            return;
        }

        const triggerElement = loadMoreTriggerRef.current;
        if (!triggerElement) return;

        const observer = new IntersectionObserver(
            (entries) => {
                if (entries[0].isIntersecting && hasMore && !isLoadingMore) {
                    onLoadMore();
                }
            },
            {
                root: null,
                rootMargin: '100px',
                threshold: 0,
            }
        );

        observer.observe(triggerElement);

        return () => observer.disconnect();
    }, [hasMore, isLoadingMore, onLoadMore, isRecording, segments.length]);

    // Scroll-based fallback for fast scrolling
    useEffect(() => {
        if (!onLoadMore || !hasMore || isLoadingMore || isRecording) return;

        const scrollElement = scrollRef.current;
        if (!scrollElement) return;

        let ticking = false;

        const handleScroll = () => {
            if (ticking || isLoadingMore || !hasMore) return;

            ticking = true;
            requestAnimationFrame(() => {
                const { scrollTop, scrollHeight, clientHeight } = scrollElement;
                const scrollBottom = scrollHeight - scrollTop - clientHeight;

                // Trigger load when within 200px of bottom
                if (scrollBottom < 200 && hasMore && !isLoadingMore) {
                    onLoadMore();
                }
                ticking = false;
            });
        };

        scrollElement.addEventListener('scroll', handleScroll, { passive: true });
        return () => scrollElement.removeEventListener('scroll', handleScroll);
    }, [onLoadMore, hasMore, isLoadingMore, isRecording]);

    // Use simple rendering for small lists, virtualization for large lists
    const useVirtualization = segments.length >= VIRTUALIZATION_THRESHOLD;

    return (
        <div className="relative h-full min-h-0">
        <div ref={scrollRef} className="flex flex-col h-full overflow-y-auto px-4 py-2">
            {/* Recording Status Bar - Sticky at top, always visible when recording */}
            <AnimatePresence>
                {isRecording && (
                    <div className="sticky top-0 z-10 bg-background pb-2">
                        <RecordingStatusBar isPaused={isPaused} />
                    </div>
                )}
            </AnimatePresence>

            {/* Content - add padding when recording to prevent overlap */}
            <div className={isRecording ? 'pt-2' : ''}>
            {segments.length === 0 ? (
                // Empty state
                <motion.div
                    initial={{ opacity: 0 }}
                    animate={{ opacity: 1 }}
                    className="text-center text-muted-foreground mt-8"
                >
                    {isRecording ? (
                        <>
                            <div className="flex items-center justify-center mb-3">
                                <div className={`w-3 h-3 rounded-full ${isPaused ? 'bg-warning' : 'bg-brand animate-pulse'}`}></div>
                            </div>
                            <p className="text-sm text-muted-foreground">
                                {isPaused ? 'Recording paused' : 'Listening for speech...'}
                            </p>
                            <p className="text-xs mt-1 text-muted-foreground/70">
                                {isPaused ? 'Click resume to continue recording' : 'Speak to see live transcription'}
                            </p>
                        </>
                    ) : (
                        <>
                            <div className="flex justify-center mb-4">
                                <Logomark size={48} />
                            </div>
                            <p className="text-lg font-semibold text-foreground">Welcome to Murmur</p>
                            <p className="text-xs mt-1">Start recording to see live transcription</p>
                        </>
                    )}
                </motion.div>
            ) : useVirtualization ? (
                // Virtualized rendering for large lists
                <>
                    <div
                        style={{
                            height: virtualizer.getTotalSize(),
                            width: "100%",
                            position: "relative",
                        }}
                    >
                        {virtualizer.getVirtualItems().map((virtualRow) => {
                            const segment = segments[virtualRow.index];
                            const isStreaming = streamingSegmentId === segment.id;

                            return (
                                <div
                                    key={segment.id}
                                    data-index={virtualRow.index}
                                    ref={virtualizer.measureElement}
                                    style={{
                                        position: "absolute",
                                        top: 0,
                                        left: 0,
                                        width: "100%",
                                        transform: `translateY(${virtualRow.start}px)`,
                                    }}
                                >
                                    <TranscriptSegment
                                        id={segment.id}
                                        timestamp={segment.timestamp}
                                        text={getDisplayText(segment)}
                                        confidence={segment.confidence}
                                        speaker={segment.speaker}
                                        isStreaming={isStreaming}
                                        showConfidence={showConfidence}
                                        editable={editable}
                                        isSelected={editable && (editMode?.selectedIds.has(segment.id) ?? false)}
                                        isEditing={editable && editMode?.editingId === segment.id}
                                        knownSpeakers={editMode?.knownSpeakers ?? []}
                                        onToggleSelect={editMode?.onToggleSelect}
                                        onStartEdit={editMode?.onStartEdit}
                                        onCommitEdit={editMode?.onCommitEdit}
                                        onCancelEdit={editMode?.onCancelEdit}
                                        onReassignRowSpeaker={editMode?.onReassignRowSpeaker}
                                        onSplit={editMode?.onSplit}
                                    />
                                </div>
                            );
                        })}
                    </div>

                    {/* Infinite scroll trigger and loading indicator */}
                    {(hasMore || isLoadingMore) && !isRecording && segments.length > 0 && (
                        <div ref={loadMoreTriggerRef} className="flex justify-center items-center py-4 mt-2">
                            {isLoadingMore ? (
                                <div className="flex items-center gap-2 text-muted-foreground">
                                    <div className="w-4 h-4 border-2 border-border border-t-muted-foreground rounded-full animate-spin" />
                                    <span className="text-sm">Loading more...</span>
                                </div>
                            ) : hasMore && totalCount > 0 ? (
                                <span className="text-sm text-muted-foreground/70">
                                    Showing {loadedCount} of {totalCount} segments
                                </span>
                            ) : null}
                        </div>
                    )}

                    {/* Listening indicator when recording */}
                    {!isStopping && isRecording && !isPaused && !isProcessing && segments.length > 0 && (
                        <motion.div
                            initial={{ opacity: 0 }}
                            animate={{ opacity: 1 }}
                            exit={{ opacity: 0 }}
                            className="flex items-center gap-2 mt-4 text-muted-foreground"
                        >
                            <div className="w-2 h-2 bg-brand rounded-full animate-pulse"></div>
                            <span className="text-sm">Listening...</span>
                        </motion.div>
                    )}
                </>
            ) : (
                // Simple rendering for small lists (better animations)
                <>
                    <div className="space-y-1">
                        {segments.map((segment) => {
                            const isStreaming = streamingSegmentId === segment.id;

                            return (
                                <motion.div
                                    key={segment.id}
                                    initial={{ opacity: 0, y: 5 }}
                                    animate={{ opacity: 1, y: 0 }}
                                    transition={{ duration: 0.15 }}
                                >
                                    <TranscriptSegment
                                        id={segment.id}
                                        timestamp={segment.timestamp}
                                        text={getDisplayText(segment)}
                                        confidence={segment.confidence}
                                        speaker={segment.speaker}
                                        isStreaming={isStreaming}
                                        showConfidence={showConfidence}
                                        editable={editable}
                                        isSelected={editable && (editMode?.selectedIds.has(segment.id) ?? false)}
                                        isEditing={editable && editMode?.editingId === segment.id}
                                        knownSpeakers={editMode?.knownSpeakers ?? []}
                                        onToggleSelect={editMode?.onToggleSelect}
                                        onStartEdit={editMode?.onStartEdit}
                                        onCommitEdit={editMode?.onCommitEdit}
                                        onCancelEdit={editMode?.onCancelEdit}
                                        onReassignRowSpeaker={editMode?.onReassignRowSpeaker}
                                        onSplit={editMode?.onSplit}
                                    />
                                </motion.div>
                            );
                        })}
                    </div>

                    {/* Infinite scroll trigger (for small lists that grow) */}
                    {(hasMore || isLoadingMore) && !isRecording && segments.length > 0 && (
                        <div ref={loadMoreTriggerRef} className="flex justify-center items-center py-4 mt-2">
                            {isLoadingMore ? (
                                <div className="flex items-center gap-2 text-muted-foreground">
                                    <div className="w-4 h-4 border-2 border-border border-t-muted-foreground rounded-full animate-spin" />
                                    <span className="text-sm">Loading more...</span>
                                </div>
                            ) : hasMore && totalCount > 0 ? (
                                <span className="text-sm text-muted-foreground/70">
                                    Showing {loadedCount} of {totalCount} segments
                                </span>
                            ) : null}
                        </div>
                    )}

                    {/* Listening indicator when recording */}
                    {!isStopping && isRecording && !isPaused && !isProcessing && segments.length > 0 && (
                        <motion.div
                            initial={{ opacity: 0 }}
                            animate={{ opacity: 1 }}
                            exit={{ opacity: 0 }}
                            className="flex items-center gap-2 mt-4 text-muted-foreground"
                        >
                            <div className="w-2 h-2 bg-brand rounded-full animate-pulse"></div>
                            <span className="text-sm">Listening...</span>
                        </motion.div>
                    )}
                </>
            )}
            </div>
        </div>

            {/* Jump back to the newest line. `isAtBottom` is measured from the DOM,
                so this stays hidden when the transcript is short enough not to
                scroll, and appears on a long meeting opened at the top — where
                auto-scroll is deliberately off and there is otherwise no way back
                down but dragging. */}
            <AnimatePresence>
                {!isAtBottom && segments.length > 0 && (
                    <motion.button
                        type="button"
                        initial={{ opacity: 0, y: 8 }}
                        animate={{ opacity: 1, y: 0 }}
                        exit={{ opacity: 0, y: 8 }}
                        transition={{ duration: 0.15 }}
                        onClick={scrollToBottom}
                        aria-label="Scroll to the latest transcript line"
                        className="absolute bottom-4 right-5 z-20 flex h-9 w-9 items-center justify-center rounded-full bg-brand text-brand-foreground shadow-lg transition-colors hover:bg-brand-hover focus:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background"
                    >
                        <ArrowDown size={16} />
                    </motion.button>
                )}
            </AnimatePresence>
        </div>
    );
};
