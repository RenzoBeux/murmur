# Meetily — Frontend (Tauri App)

The Meetily desktop application: a Next.js + Tailwind UI wrapped in Tauri 2.x, with a Rust core that handles audio capture and mixing, local Whisper/Parakeet transcription, speaker diarization, SQLite persistence, and LLM summarization.

This is the whole application — there is no separate server to run. See the [root README](../README.md) for an overview and the [Building guide](../docs/BUILDING.md) for prerequisites and GPU setup.

## Quick Reference

```bash
pnpm install

pnpm run dev          # Next.js UI only (port 3118)
pnpm tauri:dev        # full app, dev mode, GPU auto-detected
pnpm tauri:build      # production build
```

Convenience wrappers (clean rebuild + logging):

- **Windows:** `clean_run_windows.bat` (dev, accepts log level: `clean_run_windows.bat debug`) / `clean_build_windows.bat`
- **macOS/Linux:** `./clean_run.sh [debug|trace]` / `./clean_build.sh`

## Layout

```
/frontend
├── src/           # Next.js frontend (React/TypeScript)
├── src-tauri/     # Rust core: audio, STT engines, diarization, SQLite, LLM clients
│   └── migrations/  # SQLite schema migrations (run automatically at startup)
├── scripts/       # Build helpers: GPU auto-detection, sidecar staging
├── models/        # Whisper models (development)
└── public/        # Static assets
```

## Troubleshooting

- **macOS:** make scripts executable (`chmod +x clean_run.sh clean_build.sh`); grant microphone **and** screen-recording permissions (needed for system audio).
- **Windows:** build errors usually mean Visual Studio Build Tools (C++ workload) or CMake are missing; check Windows privacy settings for microphone access.
- More in the [Building guide](../docs/BUILDING.md#-troubleshooting).
