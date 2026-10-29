// Desktop-shell integration. In the Tauri app the app draws its own title
// bar and drives the window through these helpers. On macOS the system's
// window buttons stay, over the bar's left end. In the browser (dev / plain
// web) the window helpers no-op.

import { getCurrentWindow } from '@tauri-apps/api/window';
import { apiFetch, isDesktop } from './api';

export { isDesktop };

/** The system the app runs on, for the few things that differ */
export const platform: 'windows' | 'macos' | 'linux' = navigator.userAgent.includes('Windows')
  ? 'windows'
  : navigator.userAgent.includes('Mac')
    ? 'macos'
    : 'linux';

export function minimizeWindow(): void {
  if (isDesktop()) void getCurrentWindow().minimize();
}

export function toggleMaximizeWindow(): void {
  if (isDesktop()) void getCurrentWindow().toggleMaximize();
}

export function closeWindow(): void {
  if (isDesktop()) void getCurrentWindow().close();
}

/** Open an external URL: a web page, or an editor link (vscode://…). The
 * WebView cannot open custom schemes itself (window.open fails silently),
 * so the desktop asks the core's /open, which uses the system's handler.
 * Rejects when the URL could not be opened; callers say so. */
export async function openExternal(url: string): Promise<void> {
  if (!isDesktop()) {
    // noopener makes window.open return null, so there is no failure signal here
    window.open(url, '_blank', 'noopener,noreferrer');
    return;
  }
  await apiFetch('/api/open', { method: 'POST', body: { url } });
}

/** Kill browser-isms that betray the WebView: refresh keys, print, context menu. */
export function installDesktopGuards(): void {
  if (!isDesktop()) return;

  window.addEventListener('contextmenu', (e) => {
    const target = e.target as HTMLElement | null;
    // Keep the native menu where it's genuinely useful (copy/paste in fields)
    if (target?.closest('textarea, input, [contenteditable]')) return;
    e.preventDefault();
  });

  window.addEventListener('keydown', (e) => {
    const key = e.key.toLowerCase();
    if (
      e.key === 'F5' ||
      (e.ctrlKey && (key === 'r' || key === 'p' || key === 'u' || key === 'j')) ||
      (e.ctrlKey && e.shiftKey && key === 'r')
    ) {
      e.preventDefault();
    }
  });
}
