export function formatTime(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

function formatTimeShort(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
}

function formatDate(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleDateString('en-GB', { day: 'numeric', month: 'short', year: 'numeric' });
}

export function formatDateTime(iso: string): string {
  return `${formatDate(iso)}, ${formatTimeShort(iso)}`;
}

/** Compact age for lists: "now", "5m", "3h", "2d", then a short date */
export function formatRelative(iso: string): string {
  const ms = Date.now() - new Date(iso).getTime();
  const min = Math.floor(ms / 60_000);
  if (min < 1) return 'now';
  if (min < 60) return `${min}m`;
  const h = Math.floor(min / 60);
  if (h < 24) return `${h}h`;
  const d = Math.floor(h / 24);
  if (d < 7) return `${d}d`;
  return new Date(iso).toLocaleDateString('en-GB', { day: 'numeric', month: 'short' });
}

/** The same age in a sentence: "just now", "5m ago", "on 28 Oct" */
export function formatAgo(iso: string): string {
  const r = formatRelative(iso);
  return r === 'now' ? 'just now' : /^\d+[mhd]$/.test(r) ? `${r} ago` : `on ${r}`;
}

/** A project file's full path, written the way the system writes it
 * (backslashes on Windows) */
export function fullPath(root: string, relative: string): string {
  const full = `${root.replace(/[\\/]+$/, '')}/${relative}`;
  return /^([A-Za-z]:|\/\/|\\\\)/.test(root) ? full.replace(/\//g, '\\') : full;
}

export function fileName(path: string): string {
  const parts = path.split('/');
  return parts[parts.length - 1];
}
