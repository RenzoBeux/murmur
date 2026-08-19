/**
 * Human-readable recording length for the meeting lists.
 *
 * Long meetings read as `2:35h` (hours and minutes), shorter ones as `17m`, and
 * anything under a minute keeps its seconds (`42s`) so a stray 5-second
 * recording doesn't round away to nothing.
 *
 * Returns null when there is no usable duration — the backend leaves it unset
 * for meetings with no transcripts — so callers can skip rendering entirely
 * instead of showing a bogus "0m".
 */
export function formatMeetingDuration(seconds?: number | null): string | null {
  if (typeof seconds !== 'number' || !Number.isFinite(seconds) || seconds <= 0) return null;

  const totalSeconds = Math.round(seconds);
  if (totalSeconds < 60) return `${totalSeconds}s`;

  // Round to the nearest minute first so 59m 40s reads as "1:00h", not "59m".
  const totalMinutes = Math.round(totalSeconds / 60);
  if (totalMinutes < 60) return `${totalMinutes}m`;

  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  return `${hours}:${String(minutes).padStart(2, '0')}h`;
}
