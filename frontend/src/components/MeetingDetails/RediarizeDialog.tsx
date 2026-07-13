import React, { useState, useEffect, useRef, useCallback } from 'react';
import { Users, Loader2, AlertCircle, Download, Play, Pause, Tag, SkipForward } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '../ui/dialog';
import { Button } from '../ui/button';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { formatSpeaker } from '@/lib/speakerLabel';
import { CloudBadge } from '@/components/CloudBadge';

interface RediarizeDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  meetingId: string;
  onComplete?: () => void;
}

interface DiarizationProgressPayload {
  status:
    | 'starting'
    | 'running'
    | 'preparing-env'
    | 'uploading'
    | 'processing'
    | 'aligning'
    | 'done'
    | 'error';
  meeting_id?: string;
  speakers?: number;
  segments?: number;
  segments_updated?: number;
  reason?: string;
}

type DiarizationProvider = 'local' | 'local-pro' | 'pyannote';

interface ModelDownloadProgress {
  name: string;
  downloaded: number;
  total: number;
  percent: number;
}

interface RediarizationResult {
  meeting_id: string;
  speakers: number;
  segments_updated: number;
  /** Distinct diarized clusters that landed on transcript rows. */
  matched_speakers: number;
  /** Rows left as unattributed "system" (shown as "Others"). */
  leftover_segments: number;
}

/** One playable preview range for a speaker. */
interface SpeakerClip {
  start: number;
  end: number;
  text: string;
}

/** One diarized speaker with clean clip candidates to preview + name. */
interface SpeakerSample {
  speaker: string;
  /** Preview ranges, cleanest first. */
  clips: SpeakerClip[];
  segment_count: number;
  total_seconds: number;
}

const STATUS_COPY: Record<string, string> = {
  starting: 'Preparing…',
  running: 'Identifying speakers…',
  'preparing-env': 'Preparing local AI environment… (first run downloads ~1–2 GB)',
  uploading: 'Uploading audio to pyannoteAI…',
  processing: 'Diarizing in the cloud… this can take a few minutes',
  aligning: 'Applying speaker labels…',
  done: 'Done',
  error: 'Failed',
};

const statusCopy = (stage: string | null) => STATUS_COPY[stage ?? 'starting'] ?? 'Working…';

const formatDuration = (seconds: number): string => {
  const total = Math.max(0, Math.round(seconds));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return m > 0 ? `${m}m ${s}s` : `${s}s`;
};

export function RediarizeDialog({
  open,
  onOpenChange,
  meetingId,
  onComplete,
}: RediarizeDialogProps) {
  // 'setup' = choose method + run; 'naming' = optional post-diarization naming.
  const [step, setStep] = useState<'setup' | 'naming'>('setup');
  const [isProcessing, setIsProcessing] = useState(false);
  const [stage, setStage] = useState<DiarizationProgressPayload['status'] | null>(null);
  const [download, setDownload] = useState<ModelDownloadProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [numSpeakers, setNumSpeakers] = useState<string>('');
  const [numSpeakersPrefilled, setNumSpeakersPrefilled] = useState(false);
  // Whether the local user's mic is on its own channel (tagged "mic"). When so,
  // diarization masks it out, so the speaker count excludes you.
  const [micSeparated, setMicSeparated] = useState(true);
  const [provider, setProvider] = useState<DiarizationProvider>('local');
  const [hasCloudKey, setHasCloudKey] = useState(false);
  const [hasHfToken, setHasHfToken] = useState(false);

  // Naming step state.
  const [speakerSamples, setSpeakerSamples] = useState<SpeakerSample[]>([]);
  // Which of a speaker's candidate clips is currently selected (default 0).
  const [clipIndex, setClipIndex] = useState<Record<string, number>>({});
  const [names, setNames] = useState<Record<string, string>>({});
  const [attendeeSuggestions, setAttendeeSuggestions] = useState<string[]>([]);
  const [playingSpeaker, setPlayingSpeaker] = useState<string | null>(null);
  const [loadingSpeaker, setLoadingSpeaker] = useState<string | null>(null);
  const [isSavingNames, setIsSavingNames] = useState(false);

  const onCompleteRef = useRef(onComplete);
  const onOpenChangeRef = useRef(onOpenChange);
  useEffect(() => { onCompleteRef.current = onComplete; }, [onComplete]);
  useEffect(() => { onOpenChangeRef.current = onOpenChange; }, [onOpenChange]);

  // Web Audio playback for clip previews (AudioContext route avoids blob/CSP
  // issues and mirrors the app's existing useAudioPlayer).
  const audioCtxRef = useRef<AudioContext | null>(null);
  const sourceRef = useRef<AudioBufferSourceNode | null>(null);

  const stopClip = useCallback(() => {
    if (sourceRef.current) {
      try {
        sourceRef.current.onended = null;
        sourceRef.current.stop();
        sourceRef.current.disconnect();
      } catch {
        // already stopped
      }
      sourceRef.current = null;
    }
    setPlayingSpeaker(null);
  }, []);

  // Always (re)start playback of the given clip, replacing whatever plays now.
  const startClip = useCallback(
    async (speaker: string, clip: SpeakerClip) => {
      stopClip();
      setLoadingSpeaker(speaker);
      try {
        // `get_audio_clip` returns the WAV over the raw binary IPC channel, so
        // a current backend resolves this to an ArrayBuffer. Tolerate an older
        // backend build that serializes Vec<u8> as a number[] too.
        const raw = await invoke<ArrayBuffer | ArrayLike<number>>('get_audio_clip', {
          meetingId,
          start: clip.start,
          end: clip.end,
        });
        const bytes =
          raw instanceof ArrayBuffer
            ? new Uint8Array(raw)
            : Uint8Array.from(raw as ArrayLike<number>);
        if (bytes.byteLength === 0) throw new Error('Empty audio clip');

        if (!audioCtxRef.current) {
          const Ctx = window.AudioContext || (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
          audioCtxRef.current = new Ctx();
        }
        const ctx = audioCtxRef.current;
        if (ctx.state === 'suspended') await ctx.resume();

        // decodeAudioData detaches the ArrayBuffer it's given, so hand it an
        // isolated copy of exactly the clip bytes.
        const audioBuffer = await ctx.decodeAudioData(
          bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer,
        );
        const src = ctx.createBufferSource();
        src.buffer = audioBuffer;
        src.connect(ctx.destination);
        src.onended = () => {
          sourceRef.current = null;
          setPlayingSpeaker(null);
        };
        src.start(0);
        sourceRef.current = src;
        setPlayingSpeaker(speaker);
      } catch (e) {
        toast.error('Could not play sample', { description: String(e) });
        setPlayingSpeaker(null);
      } finally {
        setLoadingSpeaker(null);
      }
    },
    [meetingId, stopClip],
  );

  // Play/stop button: toggles the speaker's currently selected clip.
  const togglePlay = useCallback(
    (sample: SpeakerSample) => {
      if (playingSpeaker === sample.speaker) {
        stopClip();
        return;
      }
      const clip = sample.clips[clipIndex[sample.speaker] ?? 0];
      if (clip) void startClip(sample.speaker, clip);
    },
    [playingSpeaker, stopClip, startClip, clipIndex],
  );

  // Advance to the speaker's next candidate clip and play it right away.
  const nextClip = useCallback(
    (sample: SpeakerSample) => {
      if (sample.clips.length < 2) return;
      const next = ((clipIndex[sample.speaker] ?? 0) + 1) % sample.clips.length;
      setClipIndex((prev) => ({ ...prev, [sample.speaker]: next }));
      void startClip(sample.speaker, sample.clips[next]);
    },
    [clipIndex, startClip],
  );

  // Free the cached decode + audio graph when the dialog unmounts.
  useEffect(() => {
    return () => {
      stopClip();
      if (audioCtxRef.current) {
        audioCtxRef.current.close().catch(() => {});
        audioCtxRef.current = null;
      }
      invoke('clear_audio_clip_cache').catch(() => {});
    };
  }, [stopClip]);

  // Reset on closed → open
  const prevOpenRef = useRef(false);
  useEffect(() => {
    const wasOpen = prevOpenRef.current;
    prevOpenRef.current = open;
    if (open && !wasOpen) {
      setStep('setup');
      setIsProcessing(false);
      setStage(null);
      setDownload(null);
      setError(null);
      setNumSpeakers('');
      setNumSpeakersPrefilled(false);
      setMicSeparated(true);
      setProvider('local');
      setSpeakerSamples([]);
      setClipIndex({});
      setNames({});
      setAttendeeSuggestions([]);
      setPlayingSpeaker(null);
      setLoadingSpeaker(null);
      setIsSavingNames(false);

      // Cloud option is only offered when a pyannoteAI key is saved; Local
      // Pro needs a Hugging Face token for the gated community-1 model.
      invoke<string>('api_get_transcript_api_key', { provider: 'pyannote' })
        .then((key) => setHasCloudKey(key.trim() !== ''))
        .catch(() => setHasCloudKey(false));
      invoke<string>('api_get_transcript_api_key', { provider: 'huggingface' })
        .then((key) => setHasHfToken(key.trim() !== ''))
        .catch(() => setHasHfToken(false));

      // Prefill the speaker count from the attendees roster and reuse those
      // names as autocomplete suggestions in the naming step. Whether we
      // subtract the local user depends on if their mic is separated out
      // (masked from clustering) — if it isn't, their voice is clustered too.
      Promise.all([
        invoke<boolean>('meeting_has_mic_channel', { meetingId }).catch(() => true),
        invoke<string | null>('api_get_meeting_attendees', { meetingId }).catch(() => null),
      ])
        .then(([hasMic, attendees]) => {
          setMicSeparated(hasMic);
          if (!attendees) return;
          const raw = attendees
            .split(/[,\n;]+/)
            .map((s) => s.trim())
            .filter(Boolean);
          const uniqueCased = Array.from(
            new Map(raw.map((n) => [n.toLowerCase(), n])).values(),
          );
          setAttendeeSuggestions(uniqueCased);
          if (uniqueCased.length >= 2) {
            // Exclude yourself only when your mic is masked out of clustering.
            setNumSpeakers(String(hasMic ? uniqueCased.length - 1 : uniqueCased.length));
            setNumSpeakersPrefilled(true);
          }
        })
        .catch(() => {});
    }
  }, [open, meetingId]);

  // Listen for events while the dialog is mounted/open
  useEffect(() => {
    if (!open) return;
    const unlisteners: UnlistenFn[] = [];
    let cancelled = false;

    (async () => {
      const unlistenProgress = await listen<DiarizationProgressPayload>(
        'diarization-progress',
        (event) => {
          if (event.payload.meeting_id !== meetingId) return;
          setStage(event.payload.status);
          // Success toasts come from the invoke result in handleStart (which
          // also knows the requested speaker count); only errors matter here.
          if (event.payload.status === 'error') {
            setError(event.payload.reason ?? 'Unknown error');
            setIsProcessing(false);
          }
        },
      );
      if (cancelled) { unlistenProgress(); return; }
      unlisteners.push(unlistenProgress);

      const unlistenDownload = await listen<ModelDownloadProgress>(
        'diarization-model-download-progress',
        (event) => {
          setDownload(event.payload);
        },
      );
      if (cancelled) { unlistenDownload(); unlisteners.forEach(u => u()); return; }
      unlisteners.push(unlistenDownload);
    })();

    return () => {
      cancelled = true;
      unlisteners.forEach((u) => u());
    };
  }, [open, meetingId]);

  const handleStart = async () => {
    setIsProcessing(true);
    setError(null);
    setStage('starting');
    setDownload(null);

    try {
      const parsed = parseInt(numSpeakers, 10);
      const parsedNumSpeakers = Number.isFinite(parsed) && parsed >= 1 ? parsed : undefined;
      const result = await invoke<RediarizationResult>('rediarize_meeting', {
        meetingId,
        numSpeakers: parsedNumSpeakers,
        provider,
      });

      const matched = result.matched_speakers;
      toast.success(`Identified ${matched} speaker${matched === 1 ? '' : 's'}`);
      if (parsedNumSpeakers !== undefined && matched < parsedNumSpeakers) {
        toast.warning(
          `You entered ${parsedNumSpeakers} speakers, but only ${matched} could be confidently matched to the transcript. The remaining speech stays under “Others” — try re-running, or listen to the “Others” samples in the next step.`,
          { duration: 10000 },
        );
      }

      // IMPORTANT: do NOT refetch (onComplete) here. The meeting-details page
      // renders a full-screen loader while transcripts refetch, which unmounts
      // this whole subtree — including this dialog — closing the naming step
      // before it can appear. The transcript view already updates live via the
      // `transcript-rediarized` event; we refetch only when the dialog closes.
      let samples: SpeakerSample[] = [];
      try {
        samples = await invoke<SpeakerSample[]>('list_speaker_samples', { meetingId });
      } catch (e) {
        console.error('Failed to load speaker samples', e);
        toast.error('Could not load speakers to name', { description: String(e) });
      }

      if (samples.length === 0) {
        // Nothing to name — refetch to show the new labels, then close.
        onCompleteRef.current?.();
        onOpenChangeRef.current(false);
        return;
      }

      setSpeakerSamples(samples);
      setClipIndex({});
      setNames({});
      setStage(null);
      setIsProcessing(false);
      setStep('naming');

      // Decode the meeting audio into the clip cache in the background while the
      // user reads the dialog, so the first "play sample" click is instant
      // instead of paying the full-file decode. Fire-and-forget: the play path
      // decodes on demand too, so a failure here is harmless.
      invoke('prewarm_audio_clip_cache', { meetingId }).catch(() => {});
    } catch (err: unknown) {
      const msg = typeof err === 'string' ? err : err instanceof Error ? err.message : String(err);
      setError(msg);
      setIsProcessing(false);
    }
  };

  const stopAndClearAudio = useCallback(() => {
    stopClip();
    invoke('clear_audio_clip_cache').catch(() => {});
  }, [stopClip]);

  // Leave the naming step: optionally refresh the meeting view (to reflect the
  // new speaker labels/names) and close. Refetching is only safe here because
  // the dialog is going away, so the page's refetch-loader remount is fine.
  const closeNaming = useCallback(
    (refresh: boolean) => {
      stopAndClearAudio();
      if (refresh) onCompleteRef.current?.();
      onOpenChangeRef.current(false);
    },
    [stopAndClearAudio],
  );

  const handleSaveNames = async () => {
    setIsSavingNames(true);
    try {
      const renames = speakerSamples
        .map((s) => [s.speaker, (names[s.speaker] ?? '').trim()] as const)
        .filter(
          ([speaker, name]) =>
            name !== '' && name !== (formatSpeaker(speaker)?.label ?? speaker),
        );

      for (const [from, to] of renames) {
        await invoke('rename_meeting_speaker', {
          meetingId,
          fromSpeaker: from,
          toSpeaker: to,
        });
      }

      if (renames.length > 0) {
        toast.success(`Named ${renames.length} speaker${renames.length === 1 ? '' : 's'}`);
      }
      closeNaming(true);
    } catch (e) {
      toast.error('Could not save names', { description: String(e) });
      setIsSavingNames(false);
    }
  };

  const handleOpenChange = (next: boolean) => {
    if (!next && (isProcessing || isSavingNames)) return; // can't close mid-write
    if (!next) {
      if (step === 'naming') {
        closeNaming(true);
        return;
      }
      stopAndClearAudio();
    }
    onOpenChange(next);
  };

  const handleEscape = (e: KeyboardEvent) => {
    if (isProcessing || isSavingNames) e.preventDefault();
  };
  const handleInteractOutside = (e: Event) => {
    if (isProcessing || isSavingNames) e.preventDefault();
  };

  const showingDownload = download !== null && download.percent < 100;
  const namedCount = speakerSamples.filter(
    (s) => (names[s.speaker] ?? '').trim() !== '',
  ).length;

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        className="sm:max-w-[450px]"
        onEscapeKeyDown={handleEscape}
        onInteractOutside={handleInteractOutside}
      >
        {step === 'naming' ? (
          <>
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <Tag className="h-5 w-5 text-brand" />
                Name the speakers
              </DialogTitle>
              <DialogDescription>
                Optional — play a sample of each voice and give it a name. Names flow
                into the transcript, summaries and chat. Skip to keep the automatic labels.
              </DialogDescription>
            </DialogHeader>

            <div className="max-h-[50vh] overflow-y-auto py-2 pr-1 space-y-2">
              {speakerSamples.map((s) => {
                // The "system" bucket is leftover speech diarization couldn't
                // attribute (crosstalk, short interjections) — not a person.
                const isLeftover = s.speaker === 'system';
                const label = formatSpeaker(s.speaker);
                const isPlaying = playingSpeaker === s.speaker;
                const isLoading = loadingSpeaker === s.speaker;
                const currentIdx = clipIndex[s.speaker] ?? 0;
                const currentClip = s.clips[currentIdx];
                return (
                  <div
                    key={s.speaker}
                    className="rounded-lg border border-border p-3 space-y-2"
                  >
                    <div className="flex items-center gap-2">
                      <button
                        type="button"
                        onClick={() => togglePlay(s)}
                        disabled={isLoading || !currentClip}
                        className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-primary text-primary-foreground hover:bg-brand-hover disabled:opacity-60"
                        title={isPlaying ? 'Stop' : 'Play sample'}
                      >
                        {isLoading ? (
                          <Loader2 className="h-4 w-4 animate-spin" />
                        ) : isPlaying ? (
                          <Pause className="h-4 w-4" />
                        ) : (
                          <Play className="h-4 w-4" />
                        )}
                      </button>
                      {s.clips.length > 1 && (
                        <button
                          type="button"
                          onClick={() => nextClip(s)}
                          disabled={isLoading}
                          className="flex h-8 shrink-0 items-center gap-1 rounded-full border border-border px-2 text-xs text-muted-foreground hover:bg-accent disabled:opacity-60"
                          title="Play a different sample of this voice"
                        >
                          <SkipForward className="h-3 w-3" />
                          {currentIdx + 1}/{s.clips.length}
                        </button>
                      )}
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          {label && (
                            <span
                              className={`text-xs px-1.5 py-0.5 rounded ${label.className}`}
                            >
                              {label.label}
                            </span>
                          )}
                          <span className="text-xs text-muted-foreground">
                            {s.segment_count} segment{s.segment_count === 1 ? '' : 's'} ·{' '}
                            {formatDuration(s.total_seconds)}
                          </span>
                        </div>
                        {currentClip && currentClip.text.trim() !== '' && (
                          <p className="mt-1 truncate text-xs italic text-muted-foreground">
                            “{currentClip.text.trim()}”
                          </p>
                        )}
                      </div>
                    </div>
                    {isLeftover ? (
                      <p className="text-xs text-muted-foreground">
                        Not a single person — this is speech that couldn’t be confidently
                        attributed to one speaker, often several people talking at once.
                        It keeps the “Others” label.
                      </p>
                    ) : (
                      <input
                        type="text"
                        list="attendee-name-suggestions"
                        value={names[s.speaker] ?? ''}
                        onChange={(e) =>
                          setNames((prev) => ({ ...prev, [s.speaker]: e.target.value }))
                        }
                        placeholder={`Name (e.g. ${attendeeSuggestions[0] ?? 'Alice'})`}
                        className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
                      />
                    )}
                  </div>
                );
              })}
              {attendeeSuggestions.length > 0 && (
                <datalist id="attendee-name-suggestions">
                  {attendeeSuggestions.map((n) => (
                    <option key={n} value={n} />
                  ))}
                </datalist>
              )}
            </div>

            <DialogFooter>
              <Button
                variant="outline"
                onClick={() => closeNaming(true)}
                disabled={isSavingNames}
              >
                Skip
              </Button>
              <Button
                onClick={handleSaveNames}
                disabled={isSavingNames || namedCount === 0}
                className="bg-primary text-primary-foreground hover:bg-brand-hover"
              >
                {isSavingNames ? (
                  <>
                    <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                    Saving…
                  </>
                ) : (
                  <>
                    <Tag className="h-4 w-4 mr-2" />
                    Save names
                  </>
                )}
              </Button>
            </DialogFooter>
          </>
        ) : (
          <>
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                {error ? (
                  <>
                    <AlertCircle className="h-5 w-5 text-destructive" />
                    Speaker identification failed
                  </>
                ) : isProcessing ? (
                  <>
                    <Loader2 className="h-5 w-5 animate-spin text-brand" />
                    Identifying speakers…
                  </>
                ) : (
                  <>
                    <Users className="h-5 w-5 text-brand" />
                    Identify speakers
                  </>
                )}
              </DialogTitle>
              <DialogDescription>
                {error
                  ? 'An error occurred while identifying speakers.'
                  : isProcessing
                    ? statusCopy(stage)
                    : 'Run speaker diarization on this meeting. Existing speaker labels (mic / system or earlier speaker_N) will be replaced based on the audio.'}
              </DialogDescription>
            </DialogHeader>

            <div className="space-y-4 py-4">
              {!isProcessing && !error && (
                <div className="space-y-3">
                  <fieldset className="space-y-2">
                    <legend className="text-sm font-medium">Method</legend>
                    <label className="flex items-start gap-2 text-sm cursor-pointer">
                      <input
                        type="radio"
                        name="diarization-provider"
                        className="mt-0.5"
                        checked={provider === 'local'}
                        onChange={() => setProvider('local')}
                      />
                      <span>
                        On-device <span className="text-muted-foreground">(private)</span>
                        <CloudBadge locality="local" className="ml-1 align-middle" />
                        <span className="block text-xs text-muted-foreground">
                          Audio never leaves this machine.
                        </span>
                      </span>
                    </label>
                    <label
                      className={`flex items-start gap-2 text-sm ${
                        hasHfToken ? 'cursor-pointer' : 'cursor-not-allowed opacity-60'
                      }`}
                    >
                      <input
                        type="radio"
                        name="diarization-provider"
                        className="mt-0.5"
                        checked={provider === 'local-pro'}
                        disabled={!hasHfToken}
                        onChange={() => setProvider('local-pro')}
                      />
                      <span>
                        Local Pro <span className="text-muted-foreground">(best private option)</span>
                        <CloudBadge locality="local" className="ml-1 align-middle" />
                        <span className="block text-xs text-muted-foreground">
                          {hasHfToken
                            ? 'pyannote community-1 running fully on this machine.'
                            : 'Add your Hugging Face token in Settings → Transcript to enable.'}
                        </span>
                      </span>
                    </label>
                    <label
                      className={`flex items-start gap-2 text-sm ${
                        hasCloudKey ? 'cursor-pointer' : 'cursor-not-allowed opacity-60'
                      }`}
                    >
                      <input
                        type="radio"
                        name="diarization-provider"
                        className="mt-0.5"
                        checked={provider === 'pyannote'}
                        disabled={!hasCloudKey}
                        onChange={() => setProvider('pyannote')}
                      />
                      <span>
                        pyannoteAI cloud <span className="text-muted-foreground">(best accuracy)</span>
                        <CloudBadge locality="cloud" className="ml-1 align-middle" />
                        <span className="block text-xs text-muted-foreground">
                          {hasCloudKey
                            ? 'Uploads this meeting’s audio (your own voice silenced) to pyannote.ai.'
                            : 'Add your pyannoteAI API key in Settings → Transcript to enable.'}
                        </span>
                      </span>
                    </label>
                  </fieldset>
                  <div className="text-sm text-muted-foreground">
                    {provider === 'local' &&
                      'First run downloads ~115 MB of speaker models. Subsequent runs are fast.'}
                    {provider === 'local-pro' &&
                      'First run sets up a local AI environment (~1–2 GB download); later runs start immediately. Nothing is uploaded.'}
                    {provider === 'pyannote' &&
                      'Runs on pyannote.ai (precision-2). Uploaded audio is stored temporarily and auto-deleted within 48 hours.'}
                  </div>
                  <div className="space-y-1.5">
                    <label htmlFor="num-speakers" className="text-sm font-medium">
                      Number of {micSeparated ? 'other ' : ''}speakers{' '}
                      <span className="font-normal text-muted-foreground">
                        {micSeparated ? '(don’t count yourself)' : '(count everyone)'}
                      </span>
                    </label>
                    <input
                      id="num-speakers"
                      type="number"
                      min={1}
                      max={20}
                      inputMode="numeric"
                      placeholder="Auto-detect"
                      value={numSpeakers}
                      onChange={(e) => setNumSpeakers(e.target.value)}
                      className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
                    />
                    <p className="text-xs text-muted-foreground">
                      {micSeparated
                        ? 'Count only the other people — your own voice is on a separate channel and is left out of detection.'
                        : 'This recording isn’t channel-separated, so your own voice is detected too — count everyone, including yourself.'}
                      {numSpeakersPrefilled
                        ? ' Prefilled from this meeting’s attendees — adjust if needed.'
                        : ' Or leave blank to auto-detect.'}
                    </p>
                  </div>
                </div>
              )}

              {isProcessing && (
                <div className="space-y-3">
                  {showingDownload ? (
                    <div className="space-y-2">
                      <div className="flex items-center gap-2 text-sm">
                        <Download className="h-4 w-4 text-brand" />
                        Downloading {download!.name} model…
                      </div>
                      <div className="w-full bg-muted rounded-full h-2">
                        <div
                          className="bg-brand h-2 rounded-full transition-all duration-200"
                          style={{ width: `${Math.min(download!.percent, 100)}%` }}
                        />
                      </div>
                      <div className="text-xs text-muted-foreground font-mono tabular-nums">
                        {download!.percent}% ({Math.round(download!.downloaded / 1_048_576)} MB
                        of {Math.round(download!.total / 1_048_576)} MB)
                      </div>
                    </div>
                  ) : (
                    <>
                      <div className="flex items-center gap-2 text-sm">
                        <Loader2 className="h-4 w-4 animate-spin text-brand" />
                        {statusCopy(stage)}
                      </div>
                      <div className="w-full bg-muted rounded-full h-2 overflow-hidden">
                        <div className="bg-brand h-2 rounded-full animate-pulse w-1/2" />
                      </div>
                      <div className="text-xs text-muted-foreground">
                        {provider === 'local' &&
                          'Approximate runtime: ~1–2 min per 30 min of audio on CPU.'}
                        {provider === 'local-pro' &&
                          'First run can take a while (environment + model download); later runs take a few minutes per meeting.'}
                        {provider === 'pyannote' &&
                          'Upload is ~115 MB per meeting hour; cloud processing usually takes a few minutes.'}
                      </div>
                    </>
                  )}
                </div>
              )}

              {error && (
                <div className="bg-destructive/10 border border-destructive/40 rounded-lg p-3">
                  <p className="text-sm text-destructive">{error}</p>
                </div>
              )}
            </div>

            <DialogFooter>
              {!isProcessing && !error && (
                <>
                  <Button variant="outline" onClick={() => onOpenChange(false)}>
                    Cancel
                  </Button>
                  <Button onClick={handleStart} className="bg-primary text-primary-foreground hover:bg-brand-hover">
                    <Users className="h-4 w-4 mr-2" />
                    Identify speakers
                  </Button>
                </>
              )}
              {isProcessing && (
                <Button variant="outline" disabled>
                  <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                  Working…
                </Button>
              )}
              {error && (
                <>
                  <Button variant="outline" onClick={() => onOpenChange(false)}>
                    Close
                  </Button>
                  <Button
                    variant="outline"
                    onClick={() => {
                      setError(null);
                      setStage(null);
                    }}
                  >
                    Try again
                  </Button>
                </>
              )}
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
