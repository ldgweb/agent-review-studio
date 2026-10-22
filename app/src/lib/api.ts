// API bridge. The native Rust core handles every route in-process through the
// single Tauri `api` command — there is no HTTP server. Plain-browser Vite
// (`npm run dev`) has no API behind it; run `npm run tauri:dev` instead.

import { invoke } from '@tauri-apps/api/core';

export function isDesktop(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** Human-readable message for any rejection (no "Error: " prefix). */
export function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

interface ApiInit {
  method?: string;
  body?: unknown;
  /** Resolve with the payload even when it carries `error` — for callers that
   * read partial data sent next to the error. Default: reject with an Error
   * carrying the core's message. */
  allowError?: boolean;
}

export async function apiFetch<T>(path: string, init?: ApiInit): Promise<T> {
  if (!isDesktop()) {
    throw new Error("Agent Review Studio's API is only available in the desktop app (run `npm run tauri:dev`).");
  }
  const raw = await invoke<string>('api', {
    path: path.replace(/^\/api/, ''),
    method: init?.method ?? 'GET',
    body: init?.body !== undefined ? JSON.stringify(init.body) : null,
  });
  const data = JSON.parse(raw) as unknown;
  if (!init?.allowError && data && typeof data === 'object') {
    const error = (data as { error?: unknown }).error;
    if (error !== undefined && error !== null) throw new Error(String(error));
  }
  return data as T;
}
