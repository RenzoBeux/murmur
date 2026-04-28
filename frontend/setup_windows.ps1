# Meetily Windows dev-environment bootstrap.
# Idempotent: safe to re-run. Skips anything that's already in place.
#
# Usage (from frontend/ folder):
#     .\setup_windows.ps1
#     .\setup_windows.ps1 -SkipVulkan    # skip Vulkan SDK install (use -cuda/-cpu builds)
#     .\setup_windows.ps1 -Release       # build llama-helper in release mode

param(
    [switch]$SkipVulkan,
    [switch]$Release
)

$ErrorActionPreference = 'Stop'
$ScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot   = Split-Path -Parent $ScriptRoot

function Write-Step($msg) { Write-Host "`n=== $msg ===" -ForegroundColor Cyan }
function Write-Ok($msg)   { Write-Host "  [OK]   $msg" -ForegroundColor Green }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkGray }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor Yellow }

function Test-Cmd($name) {
    $null -ne (Get-Command $name -ErrorAction SilentlyContinue)
}

# --- 1. winget present? -------------------------------------------------------
Write-Step "Checking winget"
if (-not (Test-Cmd winget)) {
    throw "winget not found. Install 'App Installer' from the Microsoft Store, then re-run."
}
Write-Ok "winget available"

# --- 2. LLVM (libclang) for whisper-rs bindgen --------------------------------
Write-Step "Checking LLVM (libclang)"
$llvmBin = "C:\Program Files\LLVM\bin"
if (Test-Path (Join-Path $llvmBin 'libclang.dll')) {
    Write-Ok "LLVM already installed at $llvmBin"
} else {
    Write-Info "Installing LLVM via winget..."
    winget install --id LLVM.LLVM --silent --accept-package-agreements --accept-source-agreements
    if (-not (Test-Path (Join-Path $llvmBin 'libclang.dll'))) {
        throw "LLVM install reported success but libclang.dll not found at $llvmBin"
    }
    Write-Ok "LLVM installed"
}
$env:LIBCLANG_PATH = $llvmBin
Write-Info "LIBCLANG_PATH set for this session: $env:LIBCLANG_PATH"

# --- 3. Vulkan SDK (default Windows whisper-rs feature) -----------------------
Write-Step "Checking Vulkan SDK"
if ($SkipVulkan) {
    Write-Skip "Skipping Vulkan SDK (-SkipVulkan). Use 'pnpm tauri:dev:cuda' or 'pnpm tauri:dev:cpu'."
} elseif ($env:VULKAN_SDK -and (Test-Path $env:VULKAN_SDK)) {
    Write-Ok "Vulkan SDK present at $env:VULKAN_SDK"
} else {
    Write-Info "Installing Vulkan SDK via winget..."
    winget install --id KhronosGroup.VulkanSDK --silent --accept-package-agreements --accept-source-agreements
    Write-Info "VULKAN_SDK env var is set by the installer; you may need to open a NEW terminal for it to apply."
}

# --- 4. Build llama-helper and copy into src-tauri/binaries -------------------
Write-Step "Building llama-helper"
if (-not (Test-Cmd cargo)) {
    throw "cargo not found on PATH. Install Rust from https://rustup.rs/ and re-open the terminal."
}

$profile = if ($Release) { 'release' } else { 'debug' }
$cargoArgs = @('build', '-p', 'llama-helper')
if ($Release) { $cargoArgs += '--release' }

Push-Location $RepoRoot
try {
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo build -p llama-helper failed (exit $LASTEXITCODE)" }
} finally {
    Pop-Location
}

$src = Join-Path $RepoRoot "target\$profile\llama-helper.exe"
$dstDir = Join-Path $ScriptRoot 'src-tauri\binaries'
$dst = Join-Path $dstDir 'llama-helper-x86_64-pc-windows-msvc.exe'

if (-not (Test-Path $src)) {
    throw "Expected build output at $src but it doesn't exist."
}
if (-not (Test-Path $dstDir)) { New-Item -ItemType Directory -Path $dstDir | Out-Null }
Copy-Item $src $dst -Force
Write-Ok "Copied llama-helper to $dst"

# --- 5. pnpm dependencies -----------------------------------------------------
Write-Step "Installing pnpm dependencies"
if (-not (Test-Cmd pnpm)) {
    throw "pnpm not found. Install with 'npm install -g pnpm' or 'winget install pnpm.pnpm'."
}
Push-Location $ScriptRoot
try {
    & pnpm install
    if ($LASTEXITCODE -ne 0) { throw "pnpm install failed (exit $LASTEXITCODE)" }
} finally {
    Pop-Location
}
Write-Ok "Frontend deps installed"

# --- Done ---------------------------------------------------------------------
Write-Host "`nAll set." -ForegroundColor Green
Write-Host "Open a NEW terminal (so VULKAN_SDK / LIBCLANG_PATH are picked up), then:" -ForegroundColor Green
Write-Host "  cd frontend"
Write-Host "  pnpm tauri:dev          # auto-detect GPU"
Write-Host "  pnpm tauri:dev:cuda     # force NVIDIA"
Write-Host "  pnpm tauri:dev:cpu      # CPU only (also works without Vulkan SDK)"
