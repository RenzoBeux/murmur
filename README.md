<div align="center" style="border-bottom: none">
    <h1>
        <img src="docs/Meetily-6.png" style="border-radius: 10px;" />
        <br>
        Privacy-First AI Meeting Assistant
    </h1>
    <a href="LICENSE.md"><img src="https://img.shields.io/badge/License-MIT-blue" alt="License"></a>
    <img src="https://img.shields.io/badge/Supported_OS-macOS,_Windows,_Linux-white" alt="Supported OS">
    <p align="center">

A privacy-first AI meeting assistant that captures, transcribes, and summarizes meetings entirely on your machine. No cloud, no accounts, no data leaving your computer.

</p>

<p align="center">
    <img src="docs/meetily_demo.gif" width="650" alt="Meetily Demo" />
</p>

</div>

---

> This is a personal fork of [Meetily by Zackriya Solutions](https://github.com/Zackriya-Solutions/meeting-minutes), heavily restructured: the Python/FastAPI backend was removed entirely (everything runs inside the Tauri app now) and several features were added on top. See [What's different in this fork](#whats-different-in-this-fork).

## Introduction

Meetily runs entirely on your local machine. It captures your meetings (microphone + system audio), transcribes them in real time, and generates summaries — all locally. That makes it a good fit for anyone who needs to keep sensitive conversations under their own control.

- **Privacy First:** All processing happens locally on your device.
- **Cost-Effective:** Uses open-source AI models instead of expensive APIs.
- **Flexible:** Works offline and with any meeting platform (it captures system audio, not a bot in the call).
- **Customizable:** Self-hosted by definition — it's a desktop app you build and own.

## What's different in this fork

- **Backend-free architecture** — the upstream Python/FastAPI backend and Docker stack are gone. Persistence is local SQLite inside the Rust core; summaries and chat run in-process.
- **Rust-native speaker diarization** — identify who spoke, powered by sherpa-onnx, with mic masking and an optional speaker-count hint. No HuggingFace token or Python required.
- **Transcript editor** — edit segment text, merge/split segments, and reassign or rename speakers from the UI.
- **Speaker-attributed AI** — summaries and meeting chat see `Speaker: text` labels, so answers can attribute statements to people.
- **Per-meeting chat** — ask questions about a meeting, grounded in its transcript, with markdown rendering and message persistence.
- **LM Studio provider** — alongside Ollama, Claude, OpenAI, Groq, OpenRouter, custom OpenAI-compatible endpoints, and a bundled llama.cpp sidecar.
- **MCP server** — read-only [Model Context Protocol](https://modelcontextprotocol.io) access to your meetings database (`backend/mcp_server/`), so tools like Claude can query your transcripts and summaries.
- **Whisper output filtering** — reduces YouTube-style hallucinations in transcripts.
- **Markdown export** for meetings.

## Features

- **Local First:** All processing is done on your machine. No data ever leaves your computer.
- **Real-time Transcription:** Live transcript of your meeting as it happens, using **Whisper** or **Parakeet** models.
- **Speaker Diarization:** Identify and label individual speakers, locally.
- **AI-Powered Summaries & Chat:** Summarize meetings and chat with their transcripts using your choice of LLM provider.
- **Professional Audio Mixing:** Mic + system audio with RMS-based ducking and clipping prevention.
- **GPU Accelerated:** Metal/CoreML on macOS, CUDA or Vulkan on Windows/Linux.
- **Multi-Platform:** macOS, Windows, and Linux.

## Installation

Build from source (see the [Building guide](docs/BUILDING.md) for prerequisites and details):

```bash
git clone https://github.com/RenzoBeux/meetily
cd meetily/frontend
pnpm install
pnpm tauri:build          # auto-detects your GPU (CUDA/Metal/Vulkan/CPU)
```

Convenience wrappers with clean rebuild and logging:

- **Windows:** `clean_run_windows.bat` (dev) / `clean_build_windows.bat` (production)
- **macOS/Linux:** `./clean_run.sh` (dev) / `./clean_build.sh` (production)

## Key Features in Action

### 🎯 Local Transcription

Transcribe meetings entirely on your device using **Whisper** or **Parakeet** models. No cloud required.

<p align="center">
    <img src="docs/home.png" width="650" style="border-radius: 10px;" alt="Meetily home" />
</p>

### 📥 Import & Enhance

Import existing audio files to generate transcripts, or re-transcribe any recorded meeting with a different model or language, all processed locally.

<p align="center">
    <img src="docs/meetily-export.gif" width="650" style="border-radius: 10px;" alt="Import and Enhance" />
</p>

### 🤖 AI-Powered Summaries

Generate meeting summaries with your choice of AI provider. **Ollama** (local) is recommended, with support for LM Studio, Claude, Groq, OpenRouter, and OpenAI-compatible endpoints.

<p align="center">
    <img src="docs/summary.png" width="650" style="border-radius: 10px;" alt="Summary generation" />
</p>

<p align="center">
    <img src="docs/editor1.png" width="650" style="border-radius: 10px;" alt="Editor Summary generation" />
</p>

### 🔒 Privacy-First Design

All data stays on your machine. Transcription models, recordings, and transcripts are stored locally.

<p align="center">
    <img src="docs/settings.png" width="650" style="border-radius: 10px;" alt="Local Transcription and storage" />
</p>

### 🌐 Custom OpenAI Endpoint Support

Use your own OpenAI-compatible endpoint for AI summaries.

<p align="center">
    <img src="docs/custom.png" width="650" style="border-radius: 10px;" alt="Custom OpenAI Endpoint Configuration" />
</p>

### 🎙️ Professional Audio Mixing

Capture microphone and system audio simultaneously with intelligent ducking and clipping prevention.

<p align="center">
    <img src="docs/audio.png" width="650" style="border-radius: 10px;" alt="Device selection" />
</p>

## System Architecture

Meetily is a single, self-contained application built with [Tauri](https://tauri.app/): a Rust core (audio pipeline, STT engines, SQLite, LLM clients) with a Next.js frontend.

For more details, see the [Architecture documentation](docs/architecture.md) and [CLAUDE.md](CLAUDE.md).

## For Developers

You'll need Rust, Node.js, and pnpm. For detailed build instructions — including GPU acceleration setup per platform — see the [Building from Source guide](docs/BUILDING.md) and the [GPU Acceleration guide](docs/GPU_ACCELERATION.md). Contribution workflow is in [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT License — see [LICENSE.md](LICENSE.md). Original work copyright © 2024 Zackriya Solutions.

## Acknowledgments

- This project is a fork of [Meetily / meeting-minutes](https://github.com/Zackriya-Solutions/meeting-minutes) by **Zackriya Solutions** — thanks for open-sourcing an excellent foundation.
- Code was borrowed from [Whisper.cpp](https://github.com/ggerganov/whisper.cpp), [Screenpipe](https://github.com/mediar-ai/screenpipe), and [transcribe-rs](https://crates.io/crates/transcribe-rs).
- Thanks to **NVIDIA** for the **Parakeet** model, and to [istupakov](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx) for the ONNX conversion.
- Speaker diarization is powered by [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx).
