// Central source of truth for whether a chosen provider keeps data on-device or
// sends it to a third-party cloud. Used by CloudBadge and the global egress
// indicator so the app can honestly signal its "privacy-first" promise.
//
// Fail-safe rule: only providers explicitly known to run locally are 'local';
// anything unknown is treated as 'cloud' (assume data leaves the device).

export type Locality = 'local' | 'cloud';

// Providers that run entirely on the user's machine.
const LOCAL_SUMMARY: ReadonlySet<string> = new Set(['ollama', 'lmstudio', 'builtin-ai']);
const LOCAL_TRANSCRIPTION: ReadonlySet<string> = new Set(['localWhisper', 'parakeet']);
const LOCAL_DIARIZATION: ReadonlySet<string> = new Set(['local', 'local-pro']);

function isLocalhostEndpoint(endpoint?: string | null): boolean {
  if (!endpoint) return false;
  try {
    const host = new URL(endpoint).hostname.toLowerCase();
    return host === 'localhost' || host === '127.0.0.1' || host === '::1' || host === '0.0.0.0';
  } catch {
    return false;
  }
}

/**
 * Summarization/LLM provider locality. `custom-openai` depends on its endpoint —
 * localhost is treated as local, anything else (or unknown) as cloud.
 */
export function summaryLocality(
  provider: string,
  opts?: { customEndpoint?: string | null }
): Locality {
  if (provider === 'custom-openai') {
    return isLocalhostEndpoint(opts?.customEndpoint) ? 'local' : 'cloud';
  }
  return LOCAL_SUMMARY.has(provider) ? 'local' : 'cloud';
}

/** Transcription provider locality. */
export function transcriptionLocality(provider: string): Locality {
  return LOCAL_TRANSCRIPTION.has(provider) ? 'local' : 'cloud';
}

/** Speaker-diarization provider locality. */
export function diarizationLocality(provider: string): Locality {
  return LOCAL_DIARIZATION.has(provider) ? 'local' : 'cloud';
}
