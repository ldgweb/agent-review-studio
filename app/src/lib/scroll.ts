/** Scroll an element into view and pulse it — deep-link landing feedback.
 * HashRouter owns the URL hash, so in-page anchors don't work; every deep
 * link resolves through getElementById instead. */
export function scrollFlash(elementId: string): void {
  const el = document.getElementById(elementId);
  if (!el) return;
  el.scrollIntoView({ block: 'center', behavior: 'smooth' });
  el.classList.remove('flash-highlight');
  // Force a reflow so re-adding the class restarts the animation
  void el.offsetWidth;
  el.classList.add('flash-highlight');
}
