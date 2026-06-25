@echo off

echo Cleaning npm dependencies...
rd /s /q node_modules
del /f /q package-lock.json

echo Installing npm dependencies...
call pnpm install

echo Building the project...
call pnpm run tauri:build
