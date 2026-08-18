import { useRef, useState, useEffect, useCallback, RefObject } from "react";
import { Virtualizer } from "@tanstack/react-virtual";

interface UseAutoScrollProps {
    scrollRef: RefObject<HTMLDivElement | null>;
    segments: any[];
    isRecording: boolean;
    isPaused: boolean;
    activeSegmentId?: string;
    virtualizer?: Virtualizer<HTMLDivElement, Element>;
    virtualizationThreshold?: number;
    disableAutoScroll?: boolean; // Completely disable auto-scroll behavior (for meeting details page)
}

interface UseAutoScrollReturn {
    autoScroll: boolean;
    setAutoScroll: (value: boolean) => void;
    scrollToBottom: () => void;
    /**
     * Whether the view is currently parked at the bottom. Distinct from
     * `autoScroll`, which is the user's *intent* to follow new content:
     * `isAtBottom` is measured from the DOM, so it is correct on first paint
     * and while auto-scroll is disabled entirely. Drives the "go to bottom"
     * affordance. A container too short to scroll counts as at the bottom, so
     * nothing is offered when there is nowhere to go.
     */
    isAtBottom: boolean;
}

// Threshold in pixels to consider "at the bottom"
const SCROLL_THRESHOLD = 100;

/**
 * Custom hook to manage auto-scrolling behavior for transcript
 *
 * Features:
 * - Auto-scrolls to bottom when new content arrives during recording
 * - Pauses auto-scroll when user manually scrolls up
 * - Resumes auto-scroll when user scrolls back to the bottom
 *
 * @param segments - Array of transcript segments
 * @param isRecording - Whether recording is in progress
 * @param isPaused - Whether recording is paused
 * @param activeSegmentId - ID of the currently active segment
 * @returns Scroll ref, auto-scroll state, and scroll control functions
 */
export function useAutoScroll({
    scrollRef,
    segments,
    isRecording,
    isPaused,
    activeSegmentId,
    virtualizer,
    virtualizationThreshold = 10,
    disableAutoScroll = false,
}: UseAutoScrollProps): UseAutoScrollReturn {
    const useVirtualization = virtualizer && segments.length >= virtualizationThreshold;
    const [autoScroll, setAutoScroll] = useState(true);
    const [isAtBottom, setIsAtBottom] = useState(true);
    // Ref to always have current autoScroll value in effects
    const autoScrollRef = useRef(autoScroll);
    autoScrollRef.current = autoScroll;

    // Track if user has manually scrolled (to disable auto-scroll temporarily)
    const userScrolledRef = useRef(false);
    // Track if we're doing a programmatic scroll
    const isProgrammaticScrollRef = useRef(false);
    // Track previous segment count to detect new segments
    const prevSegmentCountRef = useRef(segments.length);

    /**
     * Check if the user is scrolled near the bottom
     */
    const isNearBottom = useCallback(() => {
        if (!scrollRef.current) return true;
        const { scrollTop, scrollHeight, clientHeight } = scrollRef.current;
        return scrollHeight - scrollTop - clientHeight <= SCROLL_THRESHOLD;
    }, [scrollRef]);

    /**
     * Measure whether the view sits at the bottom right now, straight from the
     * DOM rather than from tracked intent.
     */
    const measureAtBottom = useCallback(() => {
        const el = scrollRef.current;
        if (!el) return;
        const { scrollTop, scrollHeight, clientHeight } = el;
        const canScroll = scrollHeight - clientHeight > SCROLL_THRESHOLD;
        setIsAtBottom(!canScroll || scrollHeight - scrollTop - clientHeight <= SCROLL_THRESHOLD);
    }, [scrollRef]);

    /**
     * Scroll to bottom programmatically
     */
    const scrollToBottom = useCallback(() => {
        const el = scrollRef.current;
        if (!el) return;

        isProgrammaticScrollRef.current = true;
        // Ask the virtualizer first so it renders the tail rows; without this the
        // spacer's height is only an estimate for anything never scrolled to, and
        // the jump lands short of the real end.
        if (useVirtualization && virtualizer) {
            virtualizer.scrollToOffset(virtualizer.getTotalSize() + 1000, { align: "end" });
        }
        el.scrollTop = el.scrollHeight;
        userScrolledRef.current = false;
        setAutoScroll(true);
        setIsAtBottom(true);

        // Rows measure as they render, so the total size can still grow after the
        // first jump. Correct once it settles, then hand scrolling back to the user.
        setTimeout(() => {
            const current = scrollRef.current;
            if (current) current.scrollTop = current.scrollHeight;
            isProgrammaticScrollRef.current = false;
            measureAtBottom();
        }, 60);
    }, [scrollRef, useVirtualization, virtualizer, measureAtBottom]);

    // Handle scroll events to detect manual scrolling
    useEffect(() => {
        const container = scrollRef.current;
        if (!container) return;

        let scrollTimeout: ReturnType<typeof setTimeout> | null = null;

        const handleScroll = () => {
            // Skip if this is a programmatic scroll
            if (isProgrammaticScrollRef.current) {
                return;
            }

            // Button visibility is measured immediately — debouncing it by 100ms
            // makes the affordance feel laggy. Reading three layout properties is
            // cheap, and React bails out when the boolean is unchanged.
            measureAtBottom();

            // Debounce scroll handling to prevent rapid state changes
            if (scrollTimeout) {
                clearTimeout(scrollTimeout);
            }

            scrollTimeout = setTimeout(() => {
                // Check if user is near bottom
                const nearBottom = isNearBottom();

                if (nearBottom) {
                    // User scrolled to bottom - re-enable auto-scroll
                    userScrolledRef.current = false;
                    setAutoScroll(true);
                } else {
                    // User scrolled away from bottom - disable auto-scroll
                    userScrolledRef.current = true;
                    setAutoScroll(false);
                }
            }, 100);
        };

        container.addEventListener("scroll", handleScroll, { passive: true });

        return () => {
            container.removeEventListener("scroll", handleScroll);
            if (scrollTimeout) {
                clearTimeout(scrollTimeout);
            }
        };
    }, [isNearBottom, scrollRef, measureAtBottom]);

    // Re-measure when the content or the container changes size, not just on
    // scroll. Opening a long transcript never fires a scroll event, so without
    // this the view would report "at bottom" while parked at the top.
    useEffect(() => {
        measureAtBottom();

        const el = scrollRef.current;
        if (!el || typeof ResizeObserver === "undefined") return;

        const observer = new ResizeObserver(() => measureAtBottom());
        observer.observe(el);
        return () => observer.disconnect();
    }, [segments.length, measureAtBottom, scrollRef]);

    // Auto-scroll to bottom when new segments arrive during recording
    useEffect(() => {
        // EARLY RETURN: If auto-scroll is completely disabled (e.g., meeting details page)
        if (disableAutoScroll) {
            return;
        }

        const segmentCount = segments.length;
        const prevCount = prevSegmentCountRef.current;
        const hasNewSegments = segmentCount > prevCount;

        // Update the ref for next comparison
        prevSegmentCountRef.current = segmentCount;

        // Only scroll if new segments arrived AND user is currently at bottom
        // Check isNearBottom() immediately to avoid race conditions with the debounced scroll handler
        if (hasNewSegments && autoScrollRef.current && isRecording && !isPaused && segmentCount > 0) {
            // Check if user is at bottom RIGHT NOW before scrolling
            const isCurrentlyAtBottom = isNearBottom();
            if (!isCurrentlyAtBottom) {
                // User has scrolled up - don't auto-scroll
                return;
            }

            isProgrammaticScrollRef.current = true;

            if (useVirtualization && virtualizer) {
                // Use scrollToOffset with a large value to ensure we're at the bottom
                const totalSize = virtualizer.getTotalSize();
                virtualizer.scrollToOffset(totalSize + 1000, { align: "end" });

                // Also set scrollTop directly as backup after virtualizer updates
                setTimeout(() => {
                    if (scrollRef.current) {
                        scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
                    }
                }, 50);
            } else if (scrollRef.current) {
                scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
            }

            // Reset the flag after a longer delay for virtualization
            setTimeout(() => {
                isProgrammaticScrollRef.current = false;
            }, 150);
        }
    }, [segments.length, isRecording, isPaused, useVirtualization, virtualizer, scrollRef, isNearBottom, disableAutoScroll]);

    // Auto-scroll to active segment (when clicking on search results, etc.)
    useEffect(() => {
        if (activeSegmentId) {
            isProgrammaticScrollRef.current = true;

            if (useVirtualization && virtualizer) {
                const index = segments.findIndex((s: any) => s.id === activeSegmentId);
                if (index >= 0) {
                    virtualizer.scrollToIndex(index, { align: "center", behavior: "smooth" });
                }
            } else {
                const element = document.getElementById(`segment-${activeSegmentId}`);
                if (element) {
                    element.scrollIntoView({ behavior: "smooth", block: "center" });
                }
            }

            // Reset the flag after scroll animation completes
            setTimeout(() => {
                isProgrammaticScrollRef.current = false;
            }, 500);
        }
    }, [activeSegmentId, useVirtualization, virtualizer, segments]);

    return {
        autoScroll,
        setAutoScroll,
        scrollToBottom,
        isAtBottom,
    };
}
