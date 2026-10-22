import { useEffect, useRef } from 'react';

// The "?" cheat-sheet for the review workbench shortcuts. Modal: while it is
// open the review shortcuts are suspended (useReviewShortcuts), Escape closes
// it and focus returns to where it was.

const SHORTCUTS: Array<{ keys: string[]; label: string }> = [
  { keys: ['j', 'k'], label: 'Next / previous file' },
  { keys: ['n'], label: 'Next undecided file' },
  { keys: ['a'], label: 'Approve selected file' },
  { keys: ['r'], label: 'Reject selected file' },
  { keys: ['c'], label: 'Request changes + write note' },
  { keys: ['x'], label: 'Toggle context rail' },
  { keys: ['z'], label: 'Zen mode — code only' },
  { keys: ['+', '−'], label: 'Code font size' },
  { keys: ['/'], label: 'Focus file filter' },
  { keys: ['Ctrl', '↵'], label: 'Save diff comment' },
  { keys: ['?'], label: 'Toggle this overlay' },
  { keys: ['Esc'], label: 'Close' },
];

export function ShortcutOverlay({ onClose }: { onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
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
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            e.preventDefault();
            onClose();
          }
        }}
      >
        <div className="spread">
          <strong id="shortcut-overlay-title">Keyboard shortcuts</strong>
          <button className="btn btn-sm" onClick={onClose}>Esc</button>
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
