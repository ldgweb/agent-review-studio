import type { ReactNode } from 'react';

// One notice at a time. Callers pass a prioritized list; the first entry
// renders as a single 28px line — never a banner stack. Dismissal belongs to
// the owner (it clears the underlying state), so a notice whose condition
// recurs shows again.

export interface Notice {
  key: string;
  tone: 'warn' | 'info';
  icon: ReactNode;
  text: string;
  onDismiss: () => void;
}

export function NoticeLine({ notices }: { notices: Notice[] }) {
  const notice = notices[0];
  if (!notice) return null;

  return (
    <div className={`notice-line ${notice.tone}`} role="status">
      <span style={{ flexShrink: 0, display: 'inline-flex', alignItems: 'center' }}>{notice.icon}</span>
      <span className="notice-text">{notice.text}</span>
      <button
        className="linklike"
        title="Dismiss"
        aria-label="Dismiss notice"
        style={{ flexShrink: 0, color: 'var(--text-2)' }}
        onClick={notice.onDismiss}
      >
        ✕
      </button>
    </div>
  );
}
