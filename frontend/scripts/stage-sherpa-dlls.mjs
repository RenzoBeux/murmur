#!/usr/bin/env node
// Stage sherpa-onnx DLLs into src-tauri/dist-dlls/ so Tauri's resource
// validation can find them at cargo-build time.
//
// On Windows we link sherpa-onnx in `shared` mode (see Cargo.toml). The
// sherpa-onnx-sys build script downloads a prebuilt archive and unpacks it
// to the workspace target/sherpa-onnx-prebuilt/<version>/lib/. The DLLs are
// then copied next to the cargo build artifact. We pre-stage them into a
// stable directory the Tauri bundler can pick up.
//
// No-op on non-Windows platforms.

import { execSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const FRONTEND_DIR = resolve(__dirname, '..');
const SRC_TAURI = resolve(FRONTEND_DIR, 'src-tauri');
const WORKSPACE_ROOT = resolve(FRONTEND_DIR, '..');
const TARGET_DIR = resolve(WORKSPACE_ROOT, 'target');
const STAGING_DIR = resolve(SRC_TAURI, 'dist-dlls');
const REQUIRED_DLLS = ['sherpa-onnx-c-api.dll', 'onnxruntime.dll'];

if (process.platform !== 'win32') {
  console.log('[stage-sherpa-dlls] non-Windows platform, nothing to do.');
  process.exit(0);
}

function findInPrebuilt() {
  const root = join(TARGET_DIR, 'sherpa-onnx-prebuilt');
  if (!existsSync(root)) return null;
  for (const entry of readdirSync(root)) {
    const libDir = join(root, entry, 'lib');
    if (!existsSync(libDir)) continue;
    const hasAll = REQUIRED_DLLS.every((d) => existsSync(join(libDir, d)));
    if (hasAll) return libDir;
  }
  return null;
}

function findInTargetRelease() {
  const releaseDir = join(TARGET_DIR, 'release');
  if (!existsSync(releaseDir)) return null;
  const hasAll = REQUIRED_DLLS.every((d) => existsSync(join(releaseDir, d)));
  return hasAll ? releaseDir : null;
}

function dllsStagedAndFresh(sourceDir) {
  if (!existsSync(STAGING_DIR)) return false;
  for (const dll of REQUIRED_DLLS) {
    const staged = join(STAGING_DIR, dll);
    const src = join(sourceDir, dll);
    if (!existsSync(staged)) return false;
    if (statSync(src).mtimeMs > statSync(staged).mtimeMs) return false;
  }
  return true;
}

function copyAll(sourceDir) {
  mkdirSync(STAGING_DIR, { recursive: true });
  for (const dll of REQUIRED_DLLS) {
    const src = join(sourceDir, dll);
    const dst = join(STAGING_DIR, dll);
    copyFileSync(src, dst);
    console.log(`[stage-sherpa-dlls] copied ${dll}`);
  }
}

function triggerSherpaBuild() {
  console.log('[stage-sherpa-dlls] sherpa DLLs not found; triggering sherpa-onnx-sys build.rs via meetily check...');
  // sherpa-onnx-sys is not a workspace member, so cargo refuses
  // `--features shared` directly on it ("cannot specify features for packages
  // outside of workspace"). Instead invoke `cargo check` against meetily,
  // which declares sherpa-onnx with features = ["shared"] in its Cargo.toml —
  // feature unification propagates "shared" to sherpa-onnx-sys, and its
  // build.rs runs (which is what downloads the prebuilt archive).
  execSync(
    'cargo check --release -p meetily',
    { cwd: WORKSPACE_ROOT, stdio: 'inherit' },
  );
}

let sourceDir = findInPrebuilt() ?? findInTargetRelease();
if (!sourceDir) {
  triggerSherpaBuild();
  sourceDir = findInPrebuilt() ?? findInTargetRelease();
  if (!sourceDir) {
    console.error(
      '[stage-sherpa-dlls] ERROR: sherpa DLLs still missing after sherpa-onnx-sys build.',
    );
    console.error('  Looked in:');
    console.error(`    ${join(TARGET_DIR, 'sherpa-onnx-prebuilt')}/<version>/lib/`);
    console.error(`    ${join(TARGET_DIR, 'release')}/`);
    process.exit(1);
  }
}

if (dllsStagedAndFresh(sourceDir)) {
  console.log(`[stage-sherpa-dlls] DLLs already staged from ${sourceDir}, skipping copy.`);
  process.exit(0);
}

console.log(`[stage-sherpa-dlls] staging from ${sourceDir} → ${STAGING_DIR}`);
copyAll(sourceDir);
