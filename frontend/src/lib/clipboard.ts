import { isTauri } from '@tauri-apps/api/core';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';

// Write text to the clipboard, throwing on failure so callers can surface an error.
// Inside Tauri the native clipboard-manager plugin is used because the webview's
// navigator.clipboard can reject (e.g. "Document is not focused" in WebView2);
// the web API remains as the fallback for the browser-preview dev workflow.
export async function writeTextToClipboard(text: string): Promise<void> {
  if (isTauri()) {
    await writeText(text);
  } else {
    await navigator.clipboard.writeText(text);
  }
}
