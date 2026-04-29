/**
 * Map a source-faithful speaker tag (as written by the audio pipeline) to a
 * user-facing label and a Tailwind class for tinting it.
 *
 * Today the pipeline emits "mic" or "system" based on which audio stream
 * produced the speech segment. Diarization will later overwrite this with
 * per-speaker IDs (e.g. "speaker_1") which fall through to the default branch
 * and render as-is.
 */
export interface SpeakerLabel {
  label: string;
  className: string;
}

export function formatSpeaker(tag: string | undefined | null): SpeakerLabel | null {
  if (!tag) return null;
  switch (tag) {
    case 'mic':
      return { label: 'You', className: 'bg-blue-100 text-blue-700' };
    case 'system':
      return { label: 'Others', className: 'bg-purple-100 text-purple-700' };
    default:
      return { label: tag, className: 'bg-gray-100 text-gray-700' };
  }
}
