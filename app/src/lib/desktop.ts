// Desktop-shell integration. In the Tauri app the window has no native
// decorations: the app draws its own title bar and drives the window through
// these helpers. In the browser (dev / plain web) the window helpers no-op.

import { getCurrentWindow } from '@tauri-apps/api/window';
import { apiFetch, isDesktop } from './api';

export { isDesktop };

export function minimizeWindow(): void {
  if (isDesktop()) void getCurrentWindow().minimize();
}

export function toggleMaximizeWindow(): void {
  if (isDesktop()) void getCurrentWindow().toggleMaximize();
}

export function closeWindow(): void {
  if (isDesktop()) void getCurrentWindow().close();
}

/** Open an external URL (a PR page, a vscode:///cursor:// deep link). The
 * WebView cannot open custom schemes itself — window.open dies silently —
 * so the desktop routes through the core's /open (OS default handler).
 * Rejects when the URL could not be opened; callers surface that. */
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
