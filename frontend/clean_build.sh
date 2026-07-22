#!/bin/bash

# Exit on error
set -e

# Add log level selector with default to INFO
LOG_LEVEL=${1:-info}

case $LOG_LEVEL in
    info|debug|trace)
        export RUST_LOG=$LOG_LEVEL
        ;;
    *)
        echo "Invalid log level: $LOG_LEVEL. Valid options: info, debug, trace"
        exit 1
        ;;
esac

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
cd "$SCRIPT_DIR"

# Pick up cargo if it was just installed via rustup and PATH hasn't been reloaded
if ! command -v cargo &> /dev/null && [ -f "$HOME/.cargo/env" ]; then
    . "$HOME/.cargo/env"
fi

if ! command -v cargo &> /dev/null; then
    echo "❌ cargo not found. Install Rust via https://rustup.rs and re-run."
    exit 1
fi

# Check and install CMake if needed
echo "Checking CMake version..."
if ! command -v cmake &> /dev/null; then
    echo "CMake not found. Installing via Homebrew..."
    brew install cmake
else
    CMAKE_VERSION=$(cmake --version | head -n1 | cut -d" " -f3)
    if [[ "$CMAKE_VERSION" < "3.5" ]]; then
        echo "CMake version $CMAKE_VERSION is too old. Updating via Homebrew..."
        brew upgrade cmake
    fi
fi

# Clean up previous builds. The Cargo workspace target lives one level up
# from frontend/, so we clean both that and any stray src-tauri/target.
echo "Cleaning up previous builds..."
rm -rf ../target
rm -rf src-tauri/target
rm -rf src-tauri/gen
rm -f src-tauri/binaries/llama-helper-*

# Clean up npm, pnp and next
echo "Cleaning up node_modules, .next and out..."
rm -rf node_modules
rm -rf .next
rm -rf .pnp.cjs
rm -rf out

echo "Installing dependencies..."
pnpm install

# Build the llama-helper sidecar into src-tauri/binaries/llama-helper-<target-triple>
# (required by tauri.conf.json's externalBin). llama-cpp-2 has no CoreML backend,
# so macOS uses Metal.
echo "Building llama-helper sidecar..."
TARGET_TRIPLE=$(rustc -vV | grep "host:" | awk '{print $2}')
(cd .. && cargo build --release -p llama-helper --features metal)
mkdir -p src-tauri/binaries
cp ../target/release/llama-helper "src-tauri/binaries/llama-helper-$TARGET_TRIPLE"

# Run the Tauri build — tauri-auto.js auto-detects the GPU feature, and
# Tauri's beforeBuildCommand invokes `pnpm build` for the frontend.
echo "Running GPU-aware Tauri build..."
pnpm run tauri:build
