# Building Meetily from Source

Meetily is a single Tauri app — everything is built from the `frontend/` directory. The build automatically detects your GPU and enables the right acceleration backend.

## Prerequisites (all platforms)

- **Rust** — install from [rust-lang.org](https://www.rust-lang.org/tools/install)
- **Node.js** + **pnpm** — install Node from [nodejs.org](https://nodejs.org/), then `npm install -g pnpm`
- **CMake** — required to compile whisper.cpp / llama.cpp

## Quick Start

```bash
cd frontend
pnpm install

pnpm tauri:dev      # development mode with hot reload
pnpm tauri:build    # production build
```

`tauri:dev` / `tauri:build` run `scripts/tauri-auto.js`, which:

1. Detects your GPU via `scripts/auto-detect-gpu.js` (or honors `TAURI_GPU_FEATURE` if set)
2. Builds the `llama-helper` sidecar (local summarization) with matching features
3. Runs Tauri with the right `--features` flag and CMake environment

Convenience wrappers that also do a clean rebuild and set up logging:

- **Windows:** `clean_run_windows.bat` (dev) / `clean_build_windows.bat` (production)
- **macOS/Linux:** `./clean_run.sh` (dev) / `./clean_build.sh` (production)

### Forcing a specific backend

Explicit scripts exist for every backend — no auto-detection involved:

```bash
pnpm tauri:dev:cuda       pnpm tauri:build:cuda       # NVIDIA
pnpm tauri:dev:vulkan     pnpm tauri:build:vulkan     # AMD/Intel/NVIDIA
pnpm tauri:dev:metal      pnpm tauri:build:metal      # Apple Silicon
pnpm tauri:dev:hipblas    pnpm tauri:build:hipblas    # AMD ROCm
pnpm tauri:dev:openblas   pnpm tauri:build:openblas   # CPU-optimized
pnpm tauri:dev:cpu        pnpm tauri:build:cpu        # plain CPU
```

Or override auto-detection with an environment variable:

```bash
TAURI_GPU_FEATURE=cuda pnpm tauri:build     # force CUDA
TAURI_GPU_FEATURE="" pnpm tauri:build       # force CPU-only
```

> ⚠️ Never enable **CUDA and Vulkan together** — a ggml built with both backends crashes at transcription time.

## How Auto-Detection Works

| Priority | Hardware        | What It Checks                                               | Result                  |
| -------- | --------------- | ------------------------------------------------------------ | ----------------------- |
| 1️⃣       | **NVIDIA CUDA** | `nvidia-smi` exists + (`CUDA_PATH` or `nvcc` found)          | `--features cuda`       |
| 2️⃣       | **AMD ROCm**    | `rocm-smi` exists + (`ROCM_PATH` or `hipcc` found)           | `--features hipblas`    |
| 3️⃣       | **Vulkan**      | `vulkaninfo` exists + `VULKAN_SDK` + `BLAS_INCLUDE_DIRS` set | `--features vulkan`     |
| 4️⃣       | **OpenBLAS**    | `BLAS_INCLUDE_DIRS` set                                      | `--features openblas`   |
| 5️⃣       | **CPU-only**    | None of the above                                            | (no features, pure CPU) |

> 💡 **Key Insight:** GPU drivers alone aren't enough — you need the **development SDK** (CUDA toolkit, ROCm, or Vulkan SDK) installed for detection to pick a GPU backend.

For CUDA builds, `tauri-auto.js` compiles for Turing/Ampere/Ada (`CMAKE_CUDA_ARCHITECTURES=75;86;89`) by default. Pin a single architecture for much faster compiles:

```bash
CMAKE_CUDA_ARCHITECTURES=89-real pnpm tauri:build   # e.g. RTX 40xx only
```

---

## 🪟 Windows

1. Install the prerequisites above, plus **Visual Studio Build Tools** with the "Desktop development with C++" workload.
2. For NVIDIA acceleration, install the [CUDA Toolkit](https://developer.nvidia.com/cuda-downloads); for AMD/Intel, install the [Vulkan SDK](https://vulkan.lunarg.com/).
3. Build:

```powershell
cd frontend
pnpm install
clean_run_windows.bat        # dev — or: pnpm tauri:dev
clean_build_windows.bat      # production — or: pnpm tauri:build
```

Installers land in `frontend/src-tauri/target/release/bundle/` (`nsis`/`msi`).

## 🍎 macOS

Metal + CoreML acceleration is enabled automatically — no GPU setup needed.

```bash
brew install cmake node pnpm

cd frontend
pnpm install
pnpm tauri:dev      # dev
pnpm tauri:build    # production → .dmg under src-tauri/target/release/bundle/dmg/
```

## 🐧 Linux

### Basic dependencies

```bash
# Ubuntu/Debian
sudo apt update
sudo apt install build-essential cmake git

# Fedora/RHEL
sudo dnf install gcc-c++ cmake git

# Arch Linux
sudo pacman -S base-devel cmake git
```

Then `cd frontend && pnpm install && pnpm tauri:build`. Without a GPU SDK installed you get an optimized CPU build; install one of the SDKs below for acceleration.

### 🟢 NVIDIA CUDA

**Prerequisites:** NVIDIA GPU with compute capability 5.0+ (`nvidia-smi --query-gpu=compute_cap --format=csv`)

```bash
# Ubuntu/Debian (CUDA 12.x)
sudo apt install nvidia-driver-550 nvidia-cuda-toolkit

# Verify
nvidia-smi          # Shows GPU info
nvcc --version      # Shows CUDA version

# Build (auto-detects CUDA; C++17 and PIC flags are set automatically on Linux)
pnpm tauri:build
```

### 🔵 Vulkan (cross-platform fallback)

```bash
# Ubuntu/Debian
sudo apt install vulkan-sdk libopenblas-dev

# Fedora
sudo dnf install vulkan-devel openblas-devel

# Arch Linux
sudo pacman -S vulkan-devel openblas

# Required environment (add to ~/.bashrc)
export VULKAN_SDK=/usr
export BLAS_INCLUDE_DIRS=/usr/include/x86_64-linux-gnu

pnpm tauri:build
```

### 🔴 AMD ROCm

**Prerequisites:** AMD GPU with ROCm support (RX 5000+, Radeon VII, etc.)

```bash
# Add the ROCm repository first (see https://rocm.docs.amd.com)
sudo apt install rocm-smi hipcc
export ROCM_PATH=/opt/rocm

pnpm tauri:build
```

### Output

```
frontend/src-tauri/target/release/bundle/appimage/Meetily_<version>_amd64.AppImage
```

---

## 🧭 Troubleshooting

**"CUDA toolkit not found"**
- Install the CUDA toolkit (not just drivers) or set `CUDA_PATH`; `nvcc --version` should work.

**"Vulkan detected but missing dependencies"**
- Set both `VULKAN_SDK` and `BLAS_INCLUDE_DIRS` environment variables (see Vulkan section above).

**Build works but no GPU acceleration**
- Look for the GPU detection message at the top of the build output.
- Verify `nvidia-smi` (NVIDIA) or `rocm-smi` (AMD) works, and that the development SDK is installed — drivers alone are not enough.

**App crashes on every transcription**
- Make sure the build wasn't produced with both CUDA and Vulkan features enabled at once.

**CUDA build is very slow**
- Pin your GPU's architecture: `CMAKE_CUDA_ARCHITECTURES=86-real pnpm tauri:build` (RTX 30xx example).
