# Murmur Audit Remediation Plan — 2026-07-12

Companion to [`audit-2026-07-11.md`](audit-2026-07-11.md). It plans **only the work that is still missing or partial** after commit `99924d5` ("data-safety hardening"). Every finding below was re-verified against current `HEAD` (line numbers in the original audit are stale; approaches here reference the real current code).

**Already shipped by `99924d5` (not planned here):** Rust-as-writer-of-record save on stop, ffmpeg-error propagation + bounded checkpoint memory + free-disk check + `recording-error` toast, the Spanish-search UTF-8 panic fix, rotating `VACUUM INTO` backups + WAL-quarantine + non-panicking DB init, `speaker`/`currentMeetingId`/numeric-checkpoint recovery fixes, and CI (`ci.yml`).

---

## Implementation status — updated 2026-07-13 (READ THIS FIRST to continue)

Delivered as **stacked branches / PRs** off `main` (each PR bases on the previous, so the tip branch contains everything). Every wave was verified: `cargo check` 0 errors, `cargo test --lib` green (227 → 240, +13 new tests), `tsc --noEmit` 0 errors. **Runtime audio/summary/recovery flows still need a manual app run to confirm** (can't be driven headlessly).

| Wave | Status | Branch | PR |
|---|---|---|---|
| 0 — P0 data-loss holes | ✅ shipped (11 tasks) | `feat/remediation-wave-0` | #1 → `fix/data-safety-quick-wins` |
| 1 — summary/output correctness | ✅ shipped (7 tasks) | `feat/remediation-wave-1` | #2 → wave-0 |
| 2 — filesystem recovery | ✅ shipped (5 tasks) | `feat/remediation-wave-2` | #3 → wave-1 |
| 3a — recording safety (error-stop finalize + startup preflight) | ✅ shipped (2 tasks) | `feat/remediation-wave-3a` | #4 → wave-2 |
| 3b — recording trust HUD (real levels, device banners, silence watchdog, speech badge, system-audio, + toast-flood hotfix) | ✅ shipped (~8 tasks) | `feat/remediation-wave-3b` | #5 → wave-3a |
| **3c — audio-quality DSP** | ⏳ **not started** | — | — |
| **4 — security & privacy** | ⏳ **not started** | — | — |
| **5 — robustness debt (crash-safety, frontend bugs, tests)** | ⏳ **not started** | — | — |
| **6 — product features** | ⏳ **not started** | — | — |

**Repo:** GitHub `RenzoBeux/murmur` (NOT the `gh` default `Zackriya-Solutions/meetily` — always pass `--repo RenzoBeux/murmur`). The base commits `99924d5`/`e1ad60c` and all wave branches were pushed there; `main` on origin is behind at `4aca29e`.

**How to continue in a fresh session:** the tip branch `feat/remediation-wave-3b` (+ hotfix commit) has all shipped work. Start the next wave on a new branch stacked on it, keep the wave-by-wave + checkpoint cadence, PR into the previous wave branch, verify with `cargo check`/`cargo test`/`tsc`. Recommended next: **Wave 4 (security)** — self-contained, no audio-pipeline risk.

**Still to implement (in-scope, per the wave sections below):**
- **Wave 3c** — `vad-sinc-resample`, `ringbuffer-tail-flush`, `system-loudness-before-vad` (deferred from 3b: real-time audio, highest regression risk, wants a mic A/B test). *(zeropad-mic-system-desync stays P2-deferred.)*
- **Wave 4** — all 8 tasks (CSP cleanup, opener fix, download SHA-256 pinning ×3 + helper, fs-scope, cloud indicator). Keychain (4.7/4.8) is in the deferred backlog.
- **Wave 5** — 5A crash-safety (7 tasks, minus the XL `audio-ownership-thread`), 5B frontend bugs (7), 5C CI/tests (10).
- **Wave 6** — 6 product tasks (created_at, dated sidebar, language pick, title prompt, backup/restore UI, bulk export). Tags + FTS5 are deferred backlog.

**Deferred backlog (own passes — XL / schema migration / new dep):** `audio-ownership-thread` (then `auto-reconnect-backoff` + `windows-loopback-rebind`), `soft-delete-undo`, keychain, `meeting-tags`, FTS5.

**Documented follow-ups inside shipped waves:** Wave 1 — surface partial-coverage / chat-windowed truncation as **UI toasts** (needs a `SummaryOutcome` return-shape refactor; currently only logs). Wave 2 — richer `TranscriptRecovery` dialog integration (deduped startup toast is what shipped).

---

## Wave overview

| Wave | Theme | Why now | Rough size |
|---|---|---|---|
| **0** | Close remaining P0 data-loss holes | Last silent-loss paths; mostly S/M diffs | ~1 week |
| **1** | Stop shipping silently-wrong output | A "successful" summary of 5 min of a 2 h meeting is the worst non-loss failure | ~1 week |
| **2** | Complete the safety net & recovery | Filesystem recovery makes durability independent of the webview | ~1–2 weeks |
| **3** | Recording trust & audio quality | Dead mic records silence; live transcription is materially worse than import | ~2 weeks |
| **4** | Security & privacy | The app's brand promise; nothing here is done yet | ~1–2 weeks |
| **5** | Robustness & correctness debt | Crash-safety, frontend state bugs, real test coverage | ~2–3 weeks |
| **6** | Product features | "Career of meetings": findability, backup/restore, unified search | ongoing |

**Effort key:** S = <2h · M = ~half-day · L = 1–3 days · XL = multi-day/structural.

**Cross-cutting guidance**
- **Land `ci-test-harness` (Wave 5) first**, out of order, so every subsequent wave can add real tests as it goes instead of deferring them.
- **FTS5 is built once** (Wave 6 `fts5-migration`) and shared by app search, MCP search, and find-in-transcript.
- **Log-demote** appears in Wave 0; the duplicate in the security cluster is dropped.
- Each task's full design (exact code shape, risks) lives in the workflow journal that produced this plan; this doc is the sequenced index.

---

## Wave 0 — Close the remaining P0 data-loss holes

*Order: `idxdb-unsaved-guard` → `demote-transcript-logs` → `folder-init-abort` → `tray-quit-guard` + `exit-backlog-drain` (as a pair) → the four DB tasks → `merge-audio-action`. `transcription-status-fix` last.*

| # | Task | Sev / Effort | Approach (concise) | Key files | Verify |
|---|---|---|---|---|---|
| 0.1 | **`idxdb-unsaved-guard`** — never purge unsaved recovery entries | P0 / S | Gate `deleteOldMeetings` on `savedToSQLite === true`; run `checkForRecoverableTranscripts()` **before** cleanup in `performStartupChecks`; drop the 7-day upper-bound retention filter so any-age unsaved crash data is offered. | `indexedDBService.ts`, `app/page.tsx`, `useTranscriptRecovery.ts` | Keep an 8-day-old unsaved row → not deleted, appears in recovery; saved+old → cleaned. |
| 0.2 | **`demote-transcript-logs`** — stop logging transcript text at info | P1 / S | Change the transcript-interpolating `info!` lines in `whisper_engine.rs`/`worker.rs` to `perf_debug!`; make `main.rs` `RUST_LOG=info` conditional (`if is_err()`). | `whisper_engine.rs`, `transcription/worker.rs`, `main.rs` | Release build, `RUST_LOG` unset → no meeting text on stderr. |
| 0.3 | **`folder-init-abort`** — fail start cleanly if the meeting folder can't be created | P0 / M | Change `start_accumulation` to return `anyhow::Result<Sender>`; `return Err(...)` (no "continue anyway") before spawning the task; `?`-propagate through `recording_manager::start_recording`; command surfaces the Err as a toast, `IS_RECORDING` never set. | `recording_saver.rs`, `recording_manager.rs`, `recording_commands.rs` | Point recordings at a read-only path → start fails with toast, mic released, no empty folder. |
| 0.4 | **`tray-quit-guard`** — graceful stop-and-drain on Tray Quit | P0 / M | Replace `"quit" => app.exit(0)` with a handler that, if recording, awaits `stop_recording(...)` (force-flush + drain + write complete `transcripts.json`/audio) then exits; no-op-idempotent with `save_active_recording_on_exit`. | `tray.rs` | Speak, Tray→Quit, relaunch → full transcript incl. the tail is recoverable, audio finalized. |
| 0.5 | **`exit-backlog-drain`** — flush pipeline before OS-quit snapshot | P0 / M | In `save_active_recording_on_exit`, force-flush buffered pipeline audio into the transcription queue **before** snapshotting; coordinate `IS_RECORDING` clearing so tray + OS-quit don't double-save. Documented limitation: full backlog capture at OS-quit still needs the structural Rust-writes-as-produced change. | `recording_commands.rs` | Cmd+Q mid-speech → saved meeting includes the flushed buffer; no duplicate row when quitting via tray. |
| 0.6 | **`snapshot-before-migration`** — true pre-migration DB snapshot | P0 / M | Between pool-connect and `sqlx::migrate!().run()`, if there are pending migrations **and** the DB pre-existed, `VACUUM INTO backups/pre-migration/…`; share prune logic with `backup_to_dir`; best-effort (never blocks startup). | `database/manager.rs` | Add a throwaway migration → exactly one snapshot on the applying launch, none after. |
| 0.7 | **`wal-recovery-notice`** — tell the user when the WAL was quarantined | P0 / M | Capture the quarantine result into a `RecoveryNotice`, emit `database-recovered` (delayed ~600ms like `first-launch-detected`); frontend `toast.warning` with a "your newest data was set aside as `.bak`, nothing deleted" message. | `manager.rs`, `setup.rs`, `commands.rs`, `app/layout.tsx` | Corrupt a `-wal`, launch → `.bak` files created, app opens, toast shows. |
| 0.8 | **`import-copy-sidecars`** — copy `-wal`/`-shm` on legacy import | P0 / S | Helper `copy_db_with_sidecars` that also copies the `-wal`/`-shm` siblings, **renamed to match the destination stem** (`.db-wal` → `.sqlite-wal`) so SQLite associates them; use at both copy sites. | `manager.rs` | Import a legacy DB with un-checkpointed `-wal` → newest rows present. |
| 0.9 | **`import-conflict-fail-loud`** — don't silently ignore import when a DB exists | P0 / M | Explicit conflict check: if a non-empty `meeting_minutes.sqlite` exists, return a clear error (don't emit `database-initialized`); frontend shows it as an error, not false success. True merge is out of scope (documented). | `manager.rs`, `commands.rs`, `LegacyDatabaseImport.tsx` | Pre-existing meetings + import → error toast; existing DB byte-identical afterward. |
| 0.10 | **`merge-audio-action`** — one-click "merge checkpoint audio" on finalize failure | P0 / S | Include `meeting_folder` in the existing `audio_save_failed` `recording-error` emit; frontend persistent banner with a button calling `recover_audio_from_checkpoints`. | `recording_manager.rs`, `app/page.tsx`, `RecordingControls.tsx` | Break ffmpeg → banner appears; restore + click → `audio.mp4` produced and plays. |
| 0.11 | **`transcription-status-fix`** — resolve the stub vs. dead real impl | P1 / M | Give the command real data via `AtomicUsize` queue-depth + last-activity in `worker.rs`; delete the `lib.rs` stub + duplicate struct; register the real `recording_commands` version (reset counters per session). Cheap alt: just delete the dead copy. | `lib.rs`, `recording_commands.rs`, `worker.rs` | Enqueue N / complete N → depth 0, `is_processing` false; wait loop actually waits for the queue to drain. |

---

## Wave 1 — Stop shipping silently-wrong output (P1 correctness)

*Order: `ollama-native-chat` + `claude-max-tokens` first (they produce the truncation signals) → `chunk-retry-partial` (introduces the shared `SummaryOutcome { warnings }`) → `translation-persist-english` → `chat-windowing`. `pending-startup-sweep` and `fe-debounce-seq` are independent, ship anytime.*

| # | Task | Sev / Effort | Approach (concise) | Key files | Verify |
|---|---|---|---|---|---|
| 1.1 | **`ollama-native-chat`** — Ollama via native `/api/chat` with `num_ctx` | P1 / M | Add an Ollama branch that POSTs to `/api/chat` with `options.num_ctx` (from the model's real context via `ModelMetadataCache`) and `num_predict` (unbounded/user max); deserialize `prompt_eval_count`/`done_reason` to detect truncation. Non-Ollama providers unchanged. | `summary/llm_client.rs`, `processor.rs`, `service.rs`, `chat_api.rs` | 2-h transcript through Ollama now summarizes the whole meeting; `prompt_eval_count` ≫ 4096. |
| 1.2 | **`claude-max-tokens`** — parametrize Claude `max_tokens`, check `stop_reason` | P1 / S | `max_tokens.unwrap_or(8192)` instead of hardcoded 2048; add `stop_reason` to `ClaudeChatResponse`; on `"max_tokens"` push a truncation warning. | `summary/llm_client.rs` | Long Claude summary no longer cut at ~2048; `max_tokens` stop surfaces a warning. |
| 1.3 | **`chunk-retry-partial`** — retry failed chunks; mark partial not "completed" | P1 / L | Retry helper (3× exp backoff, short-circuit on "cancelled") around chunk/combine/final calls; return `SummaryOutcome { …, successful_chunks, total_chunks, warnings }`; embed a `warning` in the result JSON + `toast.warning` when coverage is partial (keep hard-fail only when *all* chunks fail). | `processor.rs`, `service.rs`, `commands.rs`, `api.rs`, `useSummaryGeneration.ts` | Kill provider between chunks → "partial summary" warning instead of a clean success. |
| 1.4 | **`translation-persist-english`** — keep the English summary if translation fails | P1 / S | In the `Translate` branch, on non-cancellation error set `final = english.clone()` + push a warning (mirror the `NormalizeEnglish` fallback) instead of `return Err` discarding it. | `processor.rs` | Force a translation failure → English summary saved + visible with a warning, not lost. |
| 1.5 | **`chat-windowing`** — head+tail transcript window + UI signal | P2 / M | Replace first-30k truncation with a head+tail slice joined by an "omitted" marker (char-safe); raise the budget for large-context Ollama via `num_ctx`; return a "condensed view" flag the chat UI shows. | `chat_api.rs`, `useSummaryGeneration.ts` | Ask about the end of a long meeting → model can answer; UI notes the condensed view. |
| 1.6 | **`pending-startup-sweep`** — reconcile orphaned PENDING summaries | P1 / M | `reset_orphaned_processes` at startup: `UPDATE … SET status='failed' … WHERE status IN ('PENDING','processing')` restoring `result_backup`; call from both DB-init entry points. Frontend: when the poller cap is hit but status is still processing, keep polling instead of erroring (kills the duplicate-generation race). | `summary.rs`, `setup.rs`, `commands.rs`, `SidebarProvider.tsx` | Force-quit mid-summary → no eternal "Generating…"; long runs don't error at ~16.5 min. |
| 1.7 | **`fe-debounce-seq`** — debounce search + latest-wins sequencing | P1 / S | `~250ms` debounce on `handleSearchChange`; a `searchSeqRef` guard so a slow earlier response can't clobber a newer one; empty-query bumps the seq and clears immediately. | `SidebarProvider.tsx`, `Sidebar/index.tsx` | Type fast → one search fires; out-of-order responses never overwrite the latest query. |

---

## Wave 2 — Complete the safety net & recovery

*Order: `rust-recovery-reader` → `rust-scan-command` ∥ `rust-import-command` → `frontend-fs-recovery` → `recovery-tests`. `soft-delete-undo` is independent (benefits from `snapshot-before-migration` landing first). `merge-audio-action` (0.10) is the smallest slice and can ship in Wave 0.*

| # | Task | Sev / Effort | Approach (concise) | Key files | Verify |
|---|---|---|---|---|---|
| 2.1 | **`rust-recovery-reader`** — tolerant readers for `metadata.json` / `transcripts.json` | P0 / M | New `audio/recovery_scan.rs`: `Value`-based readers tolerant of both writer shapes (`recording_saver` vs `common`), always carry `speaker`, sort by `sequence_id`; `InterruptedRecording` DTO. Pure/unit-testable. | `audio/recovery_scan.rs`, `audio/mod.rs` | Feed both JSON shapes + an unknown extra field → parse with speaker preserved, in order. |
| 2.2 | **`rust-scan-command`** — `scan_interrupted_recordings` (read-only, deduped) | P0 / M | Walk the recordings root(s) one level deep, pick folders with `status=="recording"`, dedup against SQLite via `list_folder_paths` (canonicalized), return `InterruptedRecording[]`. Read-only → safe to ship before the UI. | `recovery_scan.rs`, `meeting.rs`, `incremental_saver.rs`, `lib.rs` | Two folders (one recording, one completed) + a matching DB row → returns only the un-saved recording folder. |
| 2.3 | **`rust-import-command`** — `import_interrupted_recording` | P0 / M | Re-guard dedup → `save_transcript` (speaker preserved) → `recover_audio_from_checkpoints` (non-fatal) → mark folder `"recovered"` (atomic temp+rename). Returns `{ meeting_id, audio, segment_count }`. | `recovery_scan.rs`, `incremental_saver.rs`, `transcript.rs`, `lib.rs` | Import a crashed folder → meeting row + segments + `audio.mp4`; second import is a no-op. |
| 2.4 | **`frontend-fs-recovery`** — surface disk-scanned meetings in the recovery dialog | P0 / M | New `useFilesystemRecovery.ts`; run the scan at startup **independently** of IndexedDB and its 7-day window; render disk items in `TranscriptRecovery` with a "from disk" badge; dedup both sources by `folder_path`. | `useFilesystemRecovery.ts`, `app/page.tsx`, `TranscriptRecovery.tsx` | Kill app mid-record with IndexedDB cleared → dialog offers the meeting "from disk"; recover works. |
| 2.5 | **`recovery-tests`** — lock in the recovery path | P1 / M | Reader shape tests, scan dedup test (in-memory pool), import test (speaker + `recovered` status + no-op re-import). ffmpeg-optional in CI. | `recovery_scan.rs`, `meeting.rs` | `cargo test --workspace` passes; breaking the dedup filter fails a test. |
| 2.6 | **`soft-delete-undo`** — reversible delete + trash + undo toast | P0 / L | Migration `deleted_at` + index; `delete_meeting` → soft `UPDATE` (children untouched); `get_meetings`/search filter `deleted_at IS NULL`; `restore_meeting`, `purge_meeting`, `purge_trash_older_than(30)`; sidebar undo toast. **Do not** add `deleted_at` to `MeetingModel` (explicit column lists). | migration, `meeting.rs`, `api.rs`, `lib.rs`, `setup.rs`, `transcript.rs`, `Sidebar/index.tsx` | Delete → vanishes with Undo toast; Undo restores it with transcripts intact; trashed rows absent from search. |

---

## Wave 3 — Recording trust & audio quality

Covers **P0 #3 (device-failure dead code)** + **P1 #8 (live audio quality)** + the product **recording HUD** and **startup self-check**. Current-state corrections found while designing this (they differ from the stale audit): `recording-error` *is* now emitted and shown as a toast, but only **start-time** errors feed the existing red banner (`RecordingControls.tsx:528`); the level monitor actually wired in (`simple_level_monitor.rs`) emits a **fake sine wave** (the real-RMS `level_monitor.rs` runs separate cpal streams, settings-only); `speech-detected` is a **one-shot** event that's never rendered; and the Rust `AudioDeviceMonitor` already detects disconnect/reconnect but **nothing drains its event channel**.

**The spine is a single per-recording "supervisor" task** spawned at the command layer (which owns `AppHandle`), right after the manager is stored in the global. It drains device events → Tauri events, emits real RMS levels, runs the silence watchdog, drives auto-reconnect, and (Windows) polls the default output endpoint. Critical invariant everywhere below: **never hold the `RECORDING_MANAGER` std-mutex across an `.await`** (try_recv + clone, drop the guard, then emit/await). No migrations; no new crate unless the Windows re-bind uses `IMMNotificationClient` (the plan defaults to polling to avoid it).

> **Note:** the product **`recording-hud`** = tasks 3B.1–3B.3 + the banner (3A.2), not a separate item. The reconnect tasks (3A.5, 3A.6) recreate WASAPI/CoreAudio streams from tokio threads, hitting the same `unsafe impl Send` hazard as **5A.8** — sequence them after (or bundle with) the audio-ownership-thread refactor to be crash-safe.

### 3A · Device resilience (P0 #3)
*Order: `supervisor-device-events` → `device-disconnect-banner` → `error-stop-full-path` + `system-audio-unavailable-event` (independent P0s) → `auto-reconnect-backoff` → `windows-loopback-rebind` → `ring-buffer-dead-channel-detection`.*

| # | Task | Sev / Effort | Approach (concise) | Key files | Verify |
|---|---|---|---|---|---|
| 3A.1 | **`supervisor-device-events`** — drain DeviceEvents into Tauri events | P0 / M | New `audio/recording_supervisor.rs`; spawn in both start paths after `IS_RECORDING.store(true)`. ~750 ms interval: lock manager, `poll_device_events()` drains all, **drop lock**, emit `device-disconnected`/`-reconnected`/`-list-changed` (reuse `DeviceEventResponse`). Guard one-supervisor-per-session. | `recording_supervisor.rs` (new), `audio/mod.rs`, `recording_commands.rs` | Unplug USB mic mid-record → `device-disconnected` in devtools within ~2-4 s; re-plug → `device-reconnected`. |
| 3A.2 | **`device-disconnect-banner`** — render disconnect/reconnect in the existing banner | P0 / S | Add `listen('device-disconnected')` → `setDeviceError({...})`, `device-reconnected` → clear; reuse the destructive Alert at `RecordingControls.tsx:528` (no new component); clear on `recording-stopped`. | `RecordingControls.tsx` | Unplug mic → red "reconnecting" banner; re-plug → clears; gone after Stop. |
| 3A.3 | **`error-stop-full-path`** — route terminal `report_error` through full finalize | P0 / M | Stop terminating inside `RecordingState`; extract `finalize_and_stop(app)` from the stop command (flush, drain, save, finalize audio, clear `IS_RECORDING`, tray, emit); on terminal `AudioError` the callback spawns it once (guard with `compare_exchange`). **Highest-severity item** — today a terminal error leaves the app falsely "recording" and drops the meeting. | `recording_state.rs`, `recording_commands.rs`, `recording_manager.rs` | Unplug mic past the recoverable-error threshold → partial meeting saved, audio finalized, UI/tray return to idle. |
| 3A.4 | **`system-audio-unavailable-event`** — warn on mic-only fallback | P0 / S | `stream.rs` currently warns-and-continues when loopback setup fails. Track "system requested but stream inactive"; the command emits `system-audio-unavailable` (don't plumb `AppHandle` into `stream.rs`); frontend banner "recording microphone only". Recording still proceeds. | `stream.rs`, `recording_manager.rs`, `recording_commands.rs`, `RecordingControls.tsx` | Force loopback failure → banner + mic-only recording; no event when no system device selected. |
| 3A.5 | **`auto-reconnect-backoff`** — reconnect a dropped device with backoff | P0 / L | On `DeviceDisconnected`: `handle_device_disconnect`, then `attempt_device_reconnect` on 1/2/4/8/…15 s until Ok or `!IS_RECORDING`. Ownership: `take()` the manager out of the global (guard None = concurrent stop), run the `&mut`/`.await` reconnect unlocked, put it back. Keep the manual-Retry command. | `recording_supervisor.rs`, `recording_manager.rs`, `RecordingControls.tsx` | BT headset off→on → backoff attempts logged, `device-reconnected`, transcript resumes real audio. **After/with 5A.8.** |
| 3A.6 | **`windows-loopback-rebind`** — follow the default playback device | P1 / L | Windows-only: poll `playback_monitor::get_active_audio_output()` every ~2 s, 2-poll debounce; on change emit `system-device-changed` + rebind loopback via the reconnect path. Also tighten `windows.rs` to prefer exact match before `name.contains()` and surface silent default fallback. (`IMMNotificationClient` is the event-driven alt but needs the `windows` crate.) | `recording_supervisor.rs`, `devices/platform/windows.rs`, `playback_monitor.rs` | Change default playback / dock-undock mid-record → capture follows the new default (not silence). |
| 3A.7 | **`ring-buffer-dead-channel-detection`** — catch a permanently-silent channel | P1 / M | In `extract_window`, count consecutive **empty** (no chunks) windows for a channel while the other produces; after ~N s report once (recoverable `AudioError` / supervisor event). Key off buffer emptiness, never low amplitude; reset on any chunk. Defense-in-depth vs "device enumerated but delivering nothing". | `pipeline.rs`, `recording_state.rs` | Feed mic but no system chunks → starvation counter trips once; quiet-but-present stream never trips. |

### 3B · Recording HUD — real signals (this is the product `recording-hud`)
*Order: `real-rms-levels` (prerequisite) → `speech-detected-badge` → `silence-watchdog` → `startup-self-check`.*

| # | Task | Sev / Effort | Approach (concise) | Key files | Verify |
|---|---|---|---|---|---|
| 3B.1 | **`real-rms-levels`** — real waveform instead of `Math.random()` | P0 / M | Compute RMS in `pipeline.rs` after `extract_window`; store into new `AtomicU32` fields on `RecordingState` (`f32::to_bits`); supervisor emits a **new** `recording-levels` event at ~100 ms (do **not** reuse `audio-levels`); `page.tsx` maps rms→bars (perceptual/dB scaling), flatlines on silence. Retire the fake `simple_level_monitor` loop. | `pipeline.rs`, `recording_state.rs`, `recording_supervisor.rs`, `app/page.tsx`, `simple_level_monitor.rs` | Speak → bars track loudness; mute/unplug → bars flatline (previously fake). |
| 3B.2 | **`speech-detected-badge`** — live "speech detected" pill | P1 / M | Render the already-tracked `speechDetected` state; make it continuous by deriving "speaking" from `recording-levels` (rms > small threshold, ~600 ms decay) instead of the one-shot event — cheaper than touching the VAD hot path. | `RecordingControls.tsx` (+`worker.rs` only if emitting from Rust) | Badge lights while talking, self-clears ~0.6 s after silence; dead mic never lights. |
| 3B.3 | **`silence-watchdog`** — warn after N min of no speech | P1 / M | Supervisor tracks `last_signal_at` from rms; if recording & not paused & elapsed > 5 min (settings-configurable), emit `recording-silence-warning` once (latch, reset on signal); non-alarming dismissible banner. Catches the dead-mic case even when the device stays enumerated. | `recording_supervisor.rs`, `RecordingControls.tsx`, `setting.rs` (optional) | Cover the mic for the threshold → single warning; speak → resets. |
| 3B.4 | **`startup-self-check`** — ffmpeg + folder + streams-within-2s preflight | product / M | Extend `check_free_disk_space`: spawn `ffmpeg -version` (bounded), write+delete a probe file (abort on fail), post-start 2-s watchdog on a per-chunk `AtomicU64` → `recording-error` "no audio from <device>" if zero chunks. | `recording_commands.rs`, `audio/ffmpeg.rs`, `pipeline.rs` | Break ffmpeg → start aborts clearly; dead mic → 2-s "no audio" banner. |

### 3C · Live audio quality (P1 #8)
*Order: `vad-sinc-resample` → `system-loudness-before-vad` → `ringbuffer-tail-flush` → `zeropad-mic-system-desync` (P2, last).*

| # | Task | Sev / Effort | Approach (concise) | Key files | Verify |
|---|---|---|---|---|---|
| 3C.1 | **`vad-sinc-resample`** — replace the live moving-average resampler with streaming sinc | P1 / M | Give `ContinuousVadProcessor` a persistent `SincFixedIn<f32>` (same params/pattern as `AudioCapture`); buffer full chunks; drain the remainder in `flush()`. Import path untouched. | `audio/vad.rs` | Live-vs-retranscribe WER converges; anti-aliasing test (>8 kHz tone attenuated). |
| 3C.2 | **`ringbuffer-tail-flush`** — drain the trailing partial window on stop | P1 / M | `drain_partial()` on `AudioMixerRingBuffer` (zero-pad shorter side to equal length); call first in `flush_remaining_audio` → VADs + recording sender; fix the stale "50ms" comments (window is 600 ms). Idempotent. | `pipeline.rs` | Clip ending mid-word → last word now in the transcript. |
| 3C.3 | **`system-loudness-before-vad`** — normalize system audio before its VAD | P1 / M | `LoudnessNormalizer` for the system stream (mono/48k, graceful `None`); feed the normalized copy to the system VAD while the **recording** stays faithful (raw). | `pipeline.rs` | Quiet remote participant that was VAD-gated out now appears. |
| 3C.4 | **`zeropad-mic-system-desync`** — stop zero-pad drift over long meetings | P2 / L | Drain equal real-sample counts from both live streams; only pad a stream genuinely empty > N windows (absent-stream timeout so silent system audio can't stall). Highest regression risk — behind an A/B soak test; after 3C.2. | `pipeline.rs` | Asymmetric-stream test keeps mic/system within one window; silent system doesn't stall mic. |

---

## Wave 4 — Security & privacy

*Order: `remove-dead-csp-ports` + `open-external-url-opener` (cheap, instant surface reduction) → `download-hash-verify-helper` → `pin-parakeet-downloads` (in-process ONNX = realistic RCE, worst host) → `pin-diarization-and-uv-downloads` → `pin-whisper-ggml-downloads` → `keychain-secrets-store` → `keychain-migration-sweep`. `scope-webview-fs-capability` and `cloud-local-egress-indicator` slot in anytime.*

| # | Task | Sev / Effort | Approach (concise) | Key files | Verify |
|---|---|---|---|---|---|
| 4.1 | **`remove-dead-csp-ports`** — drop 5167/8178 + vestigial `serverAddress` | P2 / S | Trim CSP `connect-src` to `self`/11434/api.ollama.ai; remove `serverAddress`/`transcriptServerAddress` state, effects, and consumers; delete `config/backend_config.json` (leave `audio/capture/backend_config.rs`). | `tauri.conf.json`, `SidebarProvider.tsx`, `Sidebar/index.tsx`, `ModelSettingsModal.tsx`, `useModelConfiguration.ts`, `page-content.tsx` | tsc/eslint clean; app boots; zero refs to 5167/8178/`serverAddress`. |
| 4.2 | **`open-external-url-opener`** — kill `cmd /C start` | P2 / S | Validate scheme (`http`/`https`/`mailto`) via `url::Url`; open through `tauri-plugin-opener` (`.open_url`) + `opener` capability permission. Frontend callers unchanged. | `api.rs`, `lib.rs`, `Cargo.toml`, `tauri.conf.json` | External links open on Win/mac; injection payload opens harmlessly; `file://` rejected. |
| 4.3 | **`download-hash-verify-helper`** — shared SHA-256 verify primitive | P2 / M | `download_integrity.rs`: `verify_sha256(path, expected)` streaming through `Sha256` via `spawn_blocking`, delete-on-mismatch; `ExpectedArtifact` struct. Add `sha2`/`hex`. | `Cargo.toml`, `download_integrity.rs`, `lib.rs` | Correct digest passes; wrong digest → Err + file deleted. |
| 4.4 | **`pin-parakeet-downloads`** — hash-pin Parakeet + address fork host | P2 / L | Replace 1%-size tolerance with a per-file SHA-256 manifest, verify after each file (incl. resume), delete+error on mismatch; ideally mirror the v3 files onto a project-controlled host. | `parakeet_engine.rs` | Corrupt a hash → file rejected + model won't load; resumed-corrupt download caught. |
| 4.5 | **`pin-diarization-and-uv-downloads`** — hash-pin ONNX + version-pin uv | P2 / M | Add `SEGMENTATION_SHA256`/`EMBEDDING_SHA256`, verify (archive before extract); replace uv `releases/latest` with a pinned version + per-triple SHA-256. | `diarization/models.rs`, `diarization/localpro.rs` | Wrong hash → download rejected, diarization fails closed; uv URL has no `latest`. |
| 4.6 | **`pin-whisper-ggml-downloads`** — hash-pin GGUF models | P2 / M | Per-model SHA-256 (pin to a specific `resolve/<commit>` revision so hashes stay valid); verify after write; warn+skip unknown models. | `whisper_engine.rs` | `base` verifies; corrupt hash → rejected. |
| 4.7 | **`keychain-secrets-store`** — API keys → OS keychain | P2 / L | `database/secrets.rs` (`keyring` crate) with column-name accounts; rewrite `setting.rs` getters/setters to delegate with **SQLite fallback** (headless Linux, un-migrated keys). Signatures unchanged. | `Cargo.toml`, `secrets.rs`, `setting.rs` | Key saved → appears in Credential Manager, DB column NULL; summary still authenticates. |
| 4.8 | **`keychain-migration-sweep`** — one-time plaintext→keychain migration | P2 / M | `migrate_from_db` at startup: per column, if present and keychain empty → set keychain then NULL the DB column; idempotent; non-blocking; skip NULL-ing if keychain write fails. | `secrets.rs`, `lib.rs` | Existing plaintext keys move to keychain, DB columns NULL, providers still work. |
| 4.9 | **`scope-webview-fs-capability`** — drop `fs:read-all`/`fs:write-all` | P2 / M | Remove the blanket grants (no frontend uses `@tauri-apps/plugin-fs`); keep scoped app-data grants; run the full record→playback→export loop to confirm no regression. | `tauri.conf.json` | Record/play/export/import/download all still work; no fs permission errors. |
| 4.10 | **`cloud-local-egress-indicator`** — "leaves this device" labeling | product / M | `providerLocality.ts` (local vs cloud maps, default cloud) + `CloudBadge`; badges on settings selectors (explicit pyannote/cloud-LLM egress copy) + a persistent global indicator reflecting the active pipeline. | `providerLocality.ts`, `CloudBadge.tsx`, settings selectors, `ConfigContext.tsx` | All-local → "On device"; switch to Claude/pyannote → explicit egress warnings. |

---

## Wave 5 — Robustness & correctness debt

Three independent tracks — Rust crash-safety (P2 #10), frontend state bugs (P2 #12), and real test coverage (P2 #9). **Land `ci-test-harness` first of everything in the plan.**

### 5A · Rust crash-safety (P2 #10)
*Order: `is-recording-cas` + `read-audio-file-async` → `pipeline-new-fallible` → the two `spawn_blocking` tasks → `transcripts-jsonl` → `bound-audio-channels` → `audio-ownership-thread` last.*

| # | Task | Sev / Effort | Approach (concise) | Key files |
|---|---|---|---|---|
| 5A.1 | **`is-recording-cas`** — close the TOCTOU double-start | P2 / S | `compare_exchange(false,true)` claim at the top of both start fns; remove the later `store(true)`; reset to `false` on **every** early-Err path. | `recording_commands.rs` |
| 5A.2 | **`read-audio-file-async`** — off the main thread | P2 / S | `async fn` + `tokio::fs::read`. | `lib.rs` |
| 5A.3 | **`pipeline-new-fallible`** — no `panic!` on VAD load failure | P2 / M | `AudioPipeline::new -> anyhow::Result<Self>`; `?`-propagate; Record button shows a toast + releases the mic instead of hanging. | `pipeline.rs`, `recording_manager.rs` |
| 5A.4 | **`whisper-parakeet-spawn-blocking`** — inference off tokio workers | P2 / M | Whisper: clone the `Arc<RwLock<ctx>>`, run `create_state`/`full`/extraction inside `spawn_blocking` (state is `!Send`). Parakeet: `block_in_place` (multi-thread runtime). Re-test Windows GPU. | `whisper_engine.rs`, `parakeet_engine.rs` |
| 5A.5 | **`ffmpeg-encode-spawn-blocking`** — encode/merge off tokio workers | P2 / M | `block_in_place` around the checkpoint encode; `tokio::process::Command` for merge/finalize; error path preserved. | `encode.rs`, `incremental_saver.rs`, `recording_saver.rs` |
| 5A.6 | **`transcripts-jsonl`** — kill the O(n²) per-segment rewrite | P2 / M | Append-only `transcripts.jsonl` (`append` + `sync_all`, fold-last by `sequence_id`); keep the pretty `transcripts.json` written **once** at finalize (+`sync_all` before rename). | `recording_saver.rs`, `common.rs` |
| 5A.7 | **`bound-audio-channels`** — cap unbounded RAM growth | P2 / L | Bounded `mpsc::channel(CAP)`: realtime cpal callback uses `try_send` + drop-metric; async producers use `send().await` (backpressure); flush signals sent with `await`. | `recording_manager.rs`, `recording_saver.rs`, `pipeline.rs`, `stream.rs` |
| 5A.8 | **`audio-ownership-thread`** — remove the 4 `unsafe impl Send` | P2 / XL | Dedicated OS thread owns all cpal `Stream`s; async callers talk via a command channel + oneshot replies; delete the four `unsafe impl Send` and the dead reconnect path (replace with a `Reconnect` command). Root fix for crash-on-stop UB. | `stream.rs`, `recording_manager.rs`, new `audio/audio_thread.rs` |

### 5B · Frontend state bugs (P2 #12)
*Order: `alert-to-toast` → `recordingstatesync-istauri` → `restore-recording-status-on-reload` → `sidebar-polling-ref` → `config-loading-flag` → `refetch-overlay-not-unmount` → `live-virtualization-bounded-height` (CSS, verify last).*

| # | Task | Sev / Effort | Approach (concise) | Key files |
|---|---|---|---|---|
| 5B.1 | **`sidebar-polling-ref`** — stop clearing *all* summary polls | P2 / M | Move `activeSummaryPolls` from state `Map` into a `useRef` map; cleanup effect runs once (`[]`); remove it from the public context surface. | `SidebarProvider.tsx` |
| 5B.2 | **`refetch-overlay-not-unmount`** — don't unmount PageContent on refetch | P2 / M | Gate the full-screen spinner on `!meetingDetails` only (initial load); keep the tree mounted during refetch; optional non-blocking overlay. | `meeting-details/page.tsx`, `page-content.tsx` |
| 5B.3 | **`live-virtualization-bounded-height`** — make tanstack-virtual actually virtualize | P2 / M | Give the inner viewport a bounded height (`min-h-0`/`flex-1` chain; outer `overflow-hidden`) so the virtualizer's scroll element isn't collapsed to content height. Live panel only. | `_components/TranscriptPanel.tsx` |
| 5B.4 | **`recordingstatesync-istauri`** — fix the dead `window.__TAURI__` gate | P2 / S | Prefer **removing** the redundant 1-s poll (RecordingStateContext is the source of truth), keeping only `isRecordingDisabled`; or gate on `isTauri()`. | `useRecordingStateSync.ts` |
| 5B.5 | **`config-loading-flag`** — stop auto-summary racing config | P2 / M | Real `isModelConfigLoading` in `ConfigContext` (try/finally around the whole fetch); thread it into `useSummaryGeneration`/`SummaryPanel`; auto-generate **waits** for load instead of firing with `ollama/llama3.2`. | `ConfigContext.tsx`, `page-content.tsx` |
| 5B.6 | **`restore-recording-status-on-reload`** — restore status+polling after reload | P2 / S | In `syncWithBackend`, set `status: RECORDING` (only from IDLE/ERROR) and start the 500 ms poll when the backend reports recording. | `RecordingStateContext.tsx` |
| 5B.7 | **`alert-to-toast`** — replace blocking `alert()` | P2 / S | Swap the 5 recording-failure `alert()` sites for `toast.error(...)`. | `RecordingControls.tsx`, `useRecordingStart.ts` |

### 5C · CI & tests (P2 #9)
*Order: `ci-test-harness` first (everything depends on it); the rest are independent. Ideally the repo/pipeline/recovery tests land **with their feature wave**, not as a separate block.*

| # | Task | Sev / Effort | Approach (concise) | Key files |
|---|---|---|---|---|
| 5C.1 | **`ci-test-harness`** — shared migrated in-memory pool | P2 / S | `#[cfg(test)] migrated_pool()` (`sqlite::memory:`, `max_connections(1)`, run `sqlx::migrate!`) + insert helpers. | `database/test_support.rs`, `database/mod.rs` |
| 5C.2 | **`ci-migration-test`** — all migrations apply on a fresh DB | P2 / S | Assert key tables/columns from later migrations + `_sqlx_migrations` count == file count. | `manager.rs` |
| 5C.3 | **`ci-repo-tests-transcript`** — incl. destructive `merge_segments`/`split_segment` | P1 / M | Migrated-pool tests for merge/split/bulk-insert/bounds with atomicity assertions. | `transcript.rs` |
| 5C.4 | **`ci-repo-tests-meeting-summary-chat`** | P1 / M | Meeting cascade + title/attendees; summary status transitions + non-existent-id guard; chat add/list/clear. | `meeting.rs`, `summary.rs`, `chat.rs` |
| 5C.5 | **`ci-wal-recovery-test`** — quarantine preserves committed data | P1 / M | Extract `quarantine_wal_and_backup_main(paths)`; assert main file + `.bak` copy identical and sidecars renamed (not deleted). | `manager.rs` |
| 5C.6 | **`ci-pipeline-math-tests`** — ring-buffer mixing + normalization | P2 / M | Test `add_samples` drop, `extract_window` zero-pad, `can_mix`, stereo interleave; `normalize_v2`/loudness no-NaN/scaling. | `pipeline.rs`, `audio_processing.rs` |
| 5C.7 | **`ci-incremental-saver-tests`** — numeric sort + checkpoint scan | P1 / M | Extract `checkpoint_index`/`ordered_checkpoints`; test 999-vs-1000 ordering (lexicographic would fail); ffmpeg-free. | `incremental_saver.rs` |
| 5C.8 | **`ci-mcp-smoke-test`** — drive the real `murmur --mcp` binary | P2 / L | Integration test spawns the bin over stdio against a fixture DB; assert `initialize`/`tools/list`/`list_meetings`/`search_transcripts`. Own CI job (heavy build). | `tests/mcp_smoke.rs`, `ci.yml` |
| 5C.9 | **`ci-clippy-gate`** — make clippy fail CI | P2 / M | Clear/allow the backlog, then drop `continue-on-error` + `-D warnings` (match the test job's feature set). | `ci.yml`, `lib.rs` |
| 5C.10 | **`ci-lint-gate`** — gate eslint or document the exception | P2 / S | Measure the backlog; gate if feasible, else an explicit policy comment. tsc stays gating. | `ci.yml` |
| 5C.11 | **`ci-branch-protection-doc`** — document required checks | P2 / S | CONTRIBUTING/README: enable branch protection, mark the two job **names** required. | `CONTRIBUTING.md`, `README.md` |

---

## Wave 6 — Product features ("career of meetings")

*Order: `api-get-meetings-created-at` → `sidebar-dated-grouping` → `language-quick-pick` → `post-stop-title-prompt` → `backup-restore-ui` → `bulk-export-obsidian` → `meeting-tags` (biggest) → FTS5 unified search. Recording HUD + self-check live in Wave 3.*

| # | Task | Sev / Effort | Approach (concise) | Key files |
|---|---|---|---|---|
| 6.1 | **`api-get-meetings-created-at`** — stop stripping `created_at` | product / S | Add `created_at`/`updated_at` to the `Meeting` DTO + map (RFC3339); optional summary-status join. | `api.rs`, `SidebarProvider.tsx` |
| 6.2 | **`sidebar-dated-grouping`** — dated + day/week grouped sidebar | product / M | Carry `createdAt`; `groupMeetingsByDate` (Today/Yesterday/This Week/by month) + sticky headers + relative-date labels (local time). | `SidebarProvider.tsx`, `Sidebar/index.tsx` |
| 6.3 | **`language-quick-pick`** — Auto/EN/ES by the record button | product / S | Segmented control writing `selectedLanguage` (already live global); disable EN/ES for Parakeet. | `LanguageQuickPick.tsx`, `RecordingControls.tsx`, `app/page.tsx` |
| 6.4 | **`post-stop-title-prompt`** — AI-suggested meeting name | product / M | `api_suggest_meeting_title` (tiny `generate_summary` prompt, low max_tokens) + a skippable, non-blocking `TitlePromptDialog` after save; falls back to the default name. | `api.rs`, `lib.rs`, `useRecordingStop.ts`, `TitlePromptDialog.tsx` |
| 6.5 | **`backup-restore-ui`** — Settings › Data tab | product / M | `db_backup_now`/`db_list_backups`/`db_restore_backup` (close pool, move current aside, copy snapshot, relaunch); `DataSettings.tsx` with hard confirm. | `database/backup_commands.rs`, `manager.rs`, `lib.rs`, `SettingTabs.tsx`, `DataSettings.tsx` |
| 6.6 | **`bulk-export-obsidian`** — export all to a folder | product / M | Server-side `export_all_markdown`: folder picker, per-meeting `<title>-<date>.md` with YAML frontmatter (+tags once they exist); pure `build_meeting_markdown` helper. | `export.rs`, `lib.rs`, `DataSettings.tsx` |
| 6.7 | **`meeting-tags`** — tags/workspaces + filter chips + MCP scoping | product / L | Migration `meeting_tags`; repo CRUD (+cascade in `delete_meeting_with_transaction`); commands; sidebar chips; optional `tag` param on MCP `list_meetings`/`search_transcripts`. | migration, `meeting.rs`, `api.rs`, `lib.rs`, `mcp/tools.rs`, `SidebarProvider.tsx` |
| 6.8 | **`fts5-migration`** — FTS5 index (diacritic-insensitive, all sources) | P1 / L | Content-owning FTS5 `search_index` (`unicode61 remove_diacritics 2`) over transcripts+summaries+notes+chat(+legacy chunks); sync triggers incl. a `meetings AFTER DELETE` safety net; backfill. **Verify FTS5 is compiled into the bundled sqlite.** | migration |
| 6.9 | **`repo-search-fts`** — app search on FTS5 | P1 / M | Rewrite `search_transcripts` to `MATCH` (safe token sanitizer, `snippet()`, dedup by meeting); diacritic folding + full coverage; keep the DTO shape. | `transcript.rs`, `api.rs` |
| 6.10 | **`mcp-search-fts`** — MCP search on FTS5 | P2 / M | Same `MATCH` + shared sanitizer; **graceful fallback** to LIKE when `search_index` is missing (older DB via `--db`). | `mcp/tools.rs` |

**Deferred larger follow-ons (need their own design pass):** global-hotkey quick-record (+ pre-roll ring buffer, new `tauri-plugin-global-shortcut`), cross-meeting action-items table + "Open follow-ups" view + MCP tool, calendar-aware capture.

---

## Suggested cadence

- **Sprint 1:** Wave 0 (all) + `ci-test-harness` + `fe-debounce-seq`. Ends the remaining silent-loss paths.
- **Sprint 2:** Wave 1 (summary correctness) + Wave 2 recovery reader/scan/import.
- **Sprint 3:** Wave 3 (device wiring + HUD + audio quality) + `soft-delete-undo`.
- **Sprint 4:** Wave 4 (security) + the Wave 5C tests for everything shipped so far.
- **Sprint 5+:** Wave 5A/5B robustness (audio-ownership-thread last) and Wave 6 product features as capacity allows.
