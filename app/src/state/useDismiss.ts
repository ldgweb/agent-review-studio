import { useEffect, useRef, type RefObject } from 'react';

// Esc closes the topmost open layer, one per press: a menu, then the file
// viewer under it, say. Each layer registers while it is open. A single
// listener runs in the capture phase, so the key never also reaches the
// layers or the shortcuts underneath.

type Layer = { current: () => void };

const layers: Layer[] = [];

function onKeyDown(e: KeyboardEvent) {
  if (e.key !== 'Escape' || e.isComposing || layers.length === 0) return;
  e.preventDefault();
  e.stopPropagation();
  layers[layers.length - 1].current();
}

/** While `active`, Esc calls `onDismiss` if this is the topmost layer. With
 * `inside` (a menu, say), a click anywhere outside that element does too. */
export function useDismiss(active: boolean, onDismiss: () => void, inside?: RefObject<HTMLElement>): void {
  const layer = useRef(onDismiss);
  layer.current = onDismiss;
  useEffect(() => {
    if (!active) return;
    layers.push(layer);
    if (layers.length === 1) window.addEventListener('keydown', onKeyDown, true);
    const outside = (e: MouseEvent) => {
      if (inside?.current && !inside.current.contains(e.target as Node)) layer.current();
    };
    if (inside) window.addEventListener('mousedown', outside);
    return () => {
      const i = layers.lastIndexOf(layer);
      if (i !== -1) layers.splice(i, 1);
      if (layers.length === 0) window.removeEventListener('keydown', onKeyDown, true);
      if (inside) window.removeEventListener('mousedown', outside);
    };
  }, [active, inside]);
}
