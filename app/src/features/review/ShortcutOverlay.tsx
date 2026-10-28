import { useEffect, useRef } from 'react';
import { useDismiss } from '../../state/useDismiss';

// The "?" list of review shortcuts. Modal: while it is open the review
// shortcuts are suspended, Esc closes it and the focus returns to where it
// was.

const SHORTCUTS: Array<{ keys: string[]; label: string }> = [
  { keys: ['j', 'k'], label: 'Next / previous file' },
  { keys: ['n'], label: 'Next file to review' },
  { keys: ['a', 'r'], label: 'Approve / reject the file' },
  { keys: ['c'], label: 'Write a note about the file' },
  { keys: ['[', ']'], label: 'Previous / next change' },
  { keys: ['⇧A', '⇧R'], label: 'Approve / reject this change' },
  { keys: ['⇧C'], label: 'Comment on this change' },
  { keys: ['f'], label: 'See the whole file' },
  { keys: ['+', '−'], label: 'Text size' },
  { keys: ['Ctrl', '↵'], label: 'Save a comment' },
  { keys: ['?'], label: 'This list' },
  { keys: ['Esc'], label: 'Close' },
];

export function ShortcutOverlay({ onClose }: { onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(true, onClose);
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    ref.current?.focus();
    return () => previous?.focus?.();
  }, []);

  return (
    <div className="shortcut-backdrop" onClick={onClose}>
      <div
        ref={ref}
        className="shortcut-overlay"
        role="dialog"
        aria-modal="true"
        aria-labelledby="shortcut-overlay-title"
        tabIndex={-1}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="spread">
          <strong id="shortcut-overlay-title">Keyboard shortcuts</strong>
          <button className="btn btn-sm" title="Close (Esc)" aria-label="Close" onClick={onClose}>Esc</button>
        </div>
        <div className="mt-3">
          {SHORTCUTS.map((s) => (
            <div key={s.label} className="shortcut-row">
              <span>{s.label}</span>
              <span className="flex" style={{ gap: 4 }}>
                {s.keys.map((k) => (
                  <kbd key={k} className="kbd">{k}</kbd>
                ))}
              </span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
