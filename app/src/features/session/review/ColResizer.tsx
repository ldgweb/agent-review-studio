import { useEffect, useRef } from 'react';

// Drag handle between workbench columns. Writes the width into CSS variables
// on the .workbench root (no React re-render per pointer move) and persists
// it; double-click resets to the default. `edge` is the side of the resized
// column this handle sits on: 'right' → the column is the previous sibling,
// 'left' → the next one.

interface ColResizerProps {
  /** CSS variables to set on the parent (.workbench), e.g. ['--rail-w', '--rail-wide-w'] */
  varNames: string[];
  storageKey: string;
  min: number;
  max: number;
  edge: 'left' | 'right';
}

export function ColResizer({ varNames, storageKey, min, max, edge }: ColResizerProps) {
  const ref = useRef<HTMLDivElement>(null);

  const apply = (width: number) => {
    const parent = ref.current?.parentElement;
    if (!parent) return;
    for (const name of varNames) parent.style.setProperty(name, `${width}px`);
  };

  useEffect(() => {
    const stored = Number(localStorage.getItem(storageKey));
    if (Number.isFinite(stored) && stored >= min && stored <= max) apply(stored);
    // Run once per mount
  }, []);

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    const handle = ref.current;
    const parent = handle?.parentElement;
    if (!handle || !parent) return;
    const column = (edge === 'right' ? handle.previousElementSibling : handle.nextElementSibling) as HTMLElement | null;
    if (!column) return;
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = column.getBoundingClientRect().width;
    handle.setPointerCapture(e.pointerId);
    document.body.classList.add('col-resizing');

    let latest = startWidth;
    const onMove = (ev: PointerEvent) => {
      const delta = edge === 'right' ? ev.clientX - startX : startX - ev.clientX;
      latest = Math.round(Math.min(max, Math.max(min, startWidth + delta)));
      apply(latest);
    };
    const onUp = (ev: PointerEvent) => {
      document.body.classList.remove('col-resizing');
      localStorage.setItem(storageKey, String(latest));
      try {
        handle.releasePointerCapture(ev.pointerId);
      } catch {
        /* already released */
      }
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
  };

  const reset = () => {
    localStorage.removeItem(storageKey);
    const parent = ref.current?.parentElement;
    if (!parent) return;
    for (const name of varNames) parent.style.removeProperty(name);
  };

  return (
    <div
      ref={ref}
      className="col-resizer"
      title="Drag to resize — double-click to reset"
      onPointerDown={onPointerDown}
      onDoubleClick={reset}
    />
  );
}
