import { useEffect, useRef } from 'react';

// Keyboard-first review. Global keydown, guarded so typing in any input,
// textarea or contenteditable (e.g. the diff comment editor with its own
// Ctrl+Enter/Esc handling) is never intercepted. While a modal (the shortcut
// overlay) is open, only its own keys (? and Esc) work — pressing `a` must
// never approve the file hidden behind it.

export interface ReviewShortcutHandlers {
  nextFile: () => void;
  prevFile: () => void;
  nextUndecided: () => void;
  approve: () => void;
  reject: () => void;
  requestChanges: () => void;
  toggleRail: () => void;
  toggleZen: () => void;
  fontUp: () => void;
  fontDown: () => void;
  focusFilter: () => void;
  toggleOverlay: () => void;
  closeOverlay: () => void;
  /** A modal is open: suspend every shortcut except ? and Esc */
  suspended?: boolean;
}

export function useReviewShortcuts(handlers: ReviewShortcutHandlers): void {
  const ref = useRef(handlers);
  ref.current = handlers;

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey) return;
      const target = e.target as HTMLElement | null;
      if (target?.closest('input, textarea, select, [contenteditable="true"]')) return;
      const h = ref.current;
      if (h.suspended && e.key !== '?' && e.key !== 'Escape') return;
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
          h.requestChanges();
          break;
        case 'x':
          h.toggleRail();
          break;
        case 'z':
          h.toggleZen();
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
        case 'Escape':
          h.closeOverlay();
          break;
        default:
          return;
      }
      // Handled: keep the key from also reaching whatever it just focused
      // (e.g. `c` typing into the change-request note it opens)
      e.preventDefault();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);
}
