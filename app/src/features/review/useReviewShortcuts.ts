import { useEffect, useRef } from 'react';

// Keyboard-first review. One global keydown, ignored while you type in an
// input, a textarea or anything editable. While a modal is open (the
// shortcut list, the whole-file viewer) only ? works: pressing `a` must
// never approve the file hidden behind it. Esc belongs to whatever is open
// on top (state/useDismiss).

export interface ReviewShortcutHandlers {
  nextFile: () => void;
  prevFile: () => void;
  nextUndecided: () => void;
  approve: () => void;
  reject: () => void;
  /** Write the file's note for the agent */
  noteFile: () => void;
  /** The change (hunk) the keyboard is on */
  nextHunk: () => void;
  prevHunk: () => void;
  approveHunk: () => void;
  rejectHunk: () => void;
  /** Comment on the change the keyboard is on */
  commentHunk: () => void;
  /** See the whole file */
  viewFile: () => void;
  fontUp: () => void;
  fontDown: () => void;
  focusFilter: () => void;
  toggleOverlay: () => void;
  /** A modal is open: suspend every shortcut except ? */
  suspended?: boolean;
}

export function useReviewShortcuts(handlers: ReviewShortcutHandlers): void {
  const ref = useRef(handlers);
  ref.current = handlers;

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey) return;
      // Shift is only meaningful on the hunk keys (A / C / R)
      const target = e.target as HTMLElement | null;
      if (target?.closest('input, textarea, select, [contenteditable="true"]')) return;
      const h = ref.current;
      if (h.suspended && e.key !== '?') return;
      switch (e.key) {
        case 'j':
          h.nextFile();
          break;
        case 'k':
          h.prevFile();
          break;
        case 'n':
          h.nextUndecided();
          break;
        case 'a':
          h.approve();
          break;
        case 'r':
          h.reject();
          break;
        case 'c':
          h.noteFile();
          break;
        case ']':
          h.nextHunk();
          break;
        case '[':
          h.prevHunk();
          break;
        case 'A':
          h.approveHunk();
          break;
        case 'R':
          h.rejectHunk();
          break;
        case 'C':
          h.commentHunk();
          break;
        case 'f':
          h.viewFile();
          break;
        case '+':
        case '=':
          h.fontUp();
          break;
        case '-':
          h.fontDown();
          break;
        case '/':
          h.focusFilter();
          break;
        case '?':
          h.toggleOverlay();
          break;
        default:
          return;
      }
      // Handled: keep the key from also reaching whatever it just focused
      // (`c` would type itself into the note it opens)
      e.preventDefault();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);
}
