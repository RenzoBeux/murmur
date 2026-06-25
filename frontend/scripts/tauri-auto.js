#!/usr/bin/env node
/**
 * Auto-detect GPU and run Tauri with appropriate features
 */

const { execSync } = require('child_process');
const path = require('path');
const fs = require('fs');
const os = require('os');

// Get the command (dev or build)
const command = process.argv[2];
if (!command || !['dev', 'build'].includes(command)) {
  console.error('Usage: node tauri-auto.js [dev|build]');
  process.exit(1);
}

// Detect GPU feature
let feature = '';

// Check for environment variable override first
if (process.env.TAURI_GPU_FEATURE) {
  feature = process.env.TAURI_GPU_FEATURE;
  console.log(`🔧 Using forced GPU feature from environment: ${feature}`);
} else {
  try {
    const result = execSync('node scripts/auto-detect-gpu.js', {
      encoding: 'utf8',
      stdio: ['pipe', 'pipe', 'inherit']
    });
    feature = result.trim();
  } catch (err) {
    // If detection fails, continue with no features
  }
}

console.log(''); // Empty line for spacing

// Platform-specific environment variables
const platform = os.platform();
const env = { ...process.env };

if (feature === 'cuda') {
  // Cover current consumer GPUs: 75=Turing (RTX 20xx/GTX 16xx),
  // 86=Ampere (RTX 30xx), 89=Ada Lovelace (RTX 40xx).
  // Override with CMAKE_CUDA_ARCHITECTURES env var to pin a single arch
  // (e.g. "89-real") for faster compiles.
  env.CMAKE_CUDA_ARCHITECTURES = env.CMAKE_CUDA_ARCHITECTURES || '75;86;89';
  console.log(`CUDA arch list: ${env.CMAKE_CUDA_ARCHITECTURES}`);
  if (platform === 'linux') {
    env.CMAKE_CUDA_STANDARD = '17';
    env.CMAKE_POSITION_INDEPENDENT_CODE = 'ON';
  }
}

// Build the tauri command. For `dev`, layer tauri.dev.conf.json on top of
// tauri.conf.json so the dev build gets its own identifier (and AppData
// folder) and won't collide with an installed release build.
let tauriCmd = `tauri ${command}`;
if (command === 'dev') {
  tauriCmd += ' -c src-tauri/tauri.dev.conf.json';
}
if (feature && feature !== 'none') {
  tauriCmd += ` -- --features ${feature}`;
  console.log(`🚀 Running: tauri ${command} with features: ${feature}`);
} else {
  console.log(`🚀 Running: tauri ${command} (CPU-only mode)`);
}
console.log('');

// Execute the command
try {
  execSync(tauriCmd, { stdio: 'inherit', env });
} catch (err) {
  process.exit(err.status || 1);
}
