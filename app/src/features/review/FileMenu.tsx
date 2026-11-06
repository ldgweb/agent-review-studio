import { useEffect, useRef, type KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import type { ReviewFile } from '../../types/review';
import { platform } from '../../lib/desktop';
import { useDismiss } from '../../state/useDismiss';

// A right-click on a file in the list: the usual editor actions on it.
// Show it in the file manager, open it in your editor, copy its path.

export interface FileActions {
  editorLabel: string;
  reveal: (file: ReviewFile) => void;
  openInEditor: (file: ReviewFile) => void;
  copyPath: (file: ReviewFile, relative: boolean) => void;
}

/** Room the menu needs, to keep it inside the window */
const MENU_WIDTH = 240;
const MENU_HEIGHT = 170;
const EDGE = 8;

const REVEAL_LABEL = { windows: 'Show in File Explorer', macos: 'Show in Finder', linux: 'Show in folder' }[platform];

interface FileMenuProps {
  file: ReviewFile;
  /** Where it opens, in window coordinates */
  x: number;
  y: number;
  actions: FileActions;
  onClose: () => void;
}

export function FileMenu({ file, x, y, actions, onClose }: FileMenuProps) {
  const ref = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = onClose;
  useDismiss(true, onClose, ref);

  // The keyboard starts on the first item; leaving the window closes it
  useEffect(() => {
    ref.current?.querySelector('button')?.focus();
    const leave = () => close.current();
    window.addEventListener('blur', leave);
    window.addEventListener('resize', leave);
    return () => {
      window.removeEventListener('blur', leave);
      window.removeEventListener('resize', leave);
    };
  }, []);

  const move = (e: KeyboardEvent) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    e.preventDefault();
    const items = [...(ref.current?.querySelectorAll<HTMLButtonElement>('button') ?? [])];
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    items[(at + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length]?.focus();
  };
  const run = (action: () => void) => () => {
    onClose();
    action();
  };
  const deleted = file.changeType === 'deleted';

  return createPortal(
    <div
      ref={ref}
      className="menu context-menu"
      role="menu"
      aria-label={file.path}
      style={{ left: Math.max(EDGE, Math.min(x, window.innerWidth - MENU_WIDTH - EDGE)), top: Math.max(EDGE, Math.min(y, window.innerHeight - MENU_HEIGHT - EDGE)) }}
      onKeyDown={move}
      onContextMenu={(e) => e.preventDefault()}
    >
      <button role="menuitem" title={deleted ? 'The file is deleted: its folder opens' : undefined} onClick={run(() => actions.reveal(file))}>
        {REVEAL_LABEL}
      </button>
      {!deleted && (
        <button role="menuitem" onClick={run(() => actions.openInEditor(file))}>
          Open in {actions.editorLabel}
        </button>
      )}
      <div className="menu-sep" role="separator" />
      <button role="menuitem" onClick={run(() => actions.copyPath(file, false))}>
        Copy path
      </button>
      <button role="menuitem" onClick={run(() => actions.copyPath(file, true))}>
        Copy relative path
      </button>
    </div>,
    document.body,
  );
}
