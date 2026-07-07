# GPU Acceleration Guide

Meetily accelerates transcription (whisper.cpp via `whisper-rs`) and local summarization (llama.cpp sidecar) on the GPU. Expect roughly 5–10x faster transcription than CPU.

## Supported Backends

| Backend      | Hardware                        | Speed boost   |
| ------------ | ------------------------------- | ------------- |
| **CUDA**     | NVIDIA GPUs                     | 5–10x         |
| **Metal**    | Apple Silicon (+ CoreML layer)  | 5–10x         |
| **Vulkan**   | AMD / Intel / NVIDIA GPUs       | 3–6x          |
| **ROCm**     | AMD GPUs (`hipblas` feature)    | 4–8x          |
| **OpenBLAS** | CPU-optimized math              | 1.5–2x        |
| **CPU**      | Anything                        | 1x (baseline) |

## Automatic Detection

You normally don't configure anything: `pnpm tauri:dev` and `pnpm tauri:build` run `scripts/auto-detect-gpu.js` and pick the best available backend (CUDA → ROCm → Vulkan → OpenBLAS → CPU). Detection requires the **development SDK** for your GPU (CUDA toolkit, ROCm, or Vulkan SDK), not just drivers.

## Manual Configuration

Force a backend with the explicit scripts or the `TAURI_GPU_FEATURE` environment variable:

```bash
pnpm tauri:build:cuda                      # explicit script
TAURI_GPU_FEATURE=vulkan pnpm tauri:build  # env override
TAURI_GPU_FEATURE="" pnpm tauri:build      # force plain CPU
```

> ⚠️ Never build with **CUDA and Vulkan enabled together** — a ggml compiled with both backends aborts at transcription time.

## Platform Notes

- **macOS:** Metal + CoreML are enabled automatically; nothing to install.
- **Windows:** Install the [CUDA Toolkit](https://developer.nvidia.com/cuda-downloads) (NVIDIA) or [Vulkan SDK](https://vulkan.lunarg.com/) (AMD/Intel), plus Visual Studio Build Tools with the C++ workload.
- **Linux:** See the per-backend SDK setup (CUDA/Vulkan/ROCm) in the [Building guide](BUILDING.md#-linux).

### CUDA compile times

CUDA builds target Turing/Ampere/Ada (`75;86;89`) by default. Pin your card's architecture for much faster compiles:

```bash
nvidia-smi --query-gpu=compute_cap --format=csv   # e.g. 8.6 → "86"
CMAKE_CUDA_ARCHITECTURES=86-real pnpm tauri:build
```
