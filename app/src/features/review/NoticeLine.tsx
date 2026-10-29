// One short line above the code for what just happened ("Review copied"),
// never a stack of banners. Its owner clears it on dismiss.

export interface Notice {
  key: string;
  tone: 'warn' | 'info';
  text: string;
  onDismiss: () => void;
}

export function NoticeLine({ notice }: { notice: Notice | null }) {
  if (!notice) return null;
  return (
    <div key={notice.key} className={`notice-line ${notice.tone}`} role="status">
      <span className="notice-text">{notice.text}</span>
      <button className="icon-btn notice-close" title="Dismiss" aria-label="Dismiss" onClick={notice.onDismiss}>
        ✕
      </button>
    </div>
  );
}
