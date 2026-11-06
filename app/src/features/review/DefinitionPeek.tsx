import { useCallback, useEffect, useRef, useState, type MouseEvent as ReactMouseEvent } from 'react';
import type { DefinitionResult } from '../../types/review';
import { errorMessage } from '../../lib/api';
import { extToLang, highlightLine } from '../../lib/highlight';
import { useDismiss } from '../../state/useDismiss';

// Rest the mouse on a function or class name in the diff and its
// definition shows up next to it, whole: the comment above it and the
// body, so you can read what it does without leaving the diff. The core
// finds it by name in the work tree (read-only), so the peek says when
// more than one place matches and never claims more than that.

/** The names worth a peek: what the highlighter marks as a function or a class */
const PEEKABLE = '.token.function, .token.function-name, .token.class-name';
const IDENT = /^[A-Za-z_$][A-Za-z0-9_$]{0,79}$/;
/** How long the mouse rests on a name before the peek opens, and how long
 * it may wander off before the peek closes */
const OPEN_DELAY_MS = 550;
const CLOSE_DELAY_MS = 250;
const WIDTH = 680;

interface Peek {
  name: string;
  anchor: DOMRect;
  result?: DefinitionResult;
  error?: string;
}

/** The hover handlers for the diff, and the peek they open */
export function useDefinitionPeek(lookup: (name: string, from: string) => Promise<DefinitionResult>, from: string, reset: string) {
  const [peek, setPeek] = useState<Peek | null>(null);
  const openTimer = useRef<ReturnType<typeof setTimeout>>();
  const closeTimer = useRef<ReturnType<typeof setTimeout>>();
  const hovered = useRef<Element | null>(null);
  const cache = useRef(new Map<string, Promise<DefinitionResult>>());

  // Another file, or the code changed: what was found may be stale
  useEffect(() => {
    cache.current.clear();
    setPeek(null);
  }, [from, reset]);
  useEffect(
    () => () => {
      clearTimeout(openTimer.current);
      clearTimeout(closeTimer.current);
    },
    [],
  );

  const close = useCallback(() => {
    clearTimeout(openTimer.current);
    hovered.current = null;
    setPeek(null);
  }, []);
  const keep = useCallback(() => clearTimeout(closeTimer.current), []);
  const leave = useCallback(() => {
    clearTimeout(closeTimer.current);
    closeTimer.current = setTimeout(close, CLOSE_DELAY_MS);
  }, [close]);

  const open = useCallback(
    (el: Element) => {
      const name = el.textContent?.trim() ?? '';
      if (!IDENT.test(name)) return;
      keep();
      const anchor = el.getBoundingClientRect();
      setPeek({ name, anchor });
      let found = cache.current.get(name);
      if (!found) {
        found = lookup(name, from);
        cache.current.set(name, found);
        found.catch(() => cache.current.delete(name));
      }
      found.then(
        (result) => setPeek((p) => (p?.name === name ? { ...p, result } : p)),
        (e: unknown) => setPeek((p) => (p?.name === name ? { ...p, error: errorMessage(e) } : p)),
      );
    },
    [from, lookup, keep],
  );

  const onMouseOver = useCallback(
    (e: ReactMouseEvent) => {
      const el = (e.target as Element).closest?.(PEEKABLE) ?? null;
      if (el === hovered.current) return;
      hovered.current = el;
      clearTimeout(openTimer.current);
      if (!el) {
        leave();
        return;
      }
      keep();
      openTimer.current = setTimeout(() => open(el), OPEN_DELAY_MS);
    },
    [open, keep, leave],
  );
  const onMouseLeave = useCallback(() => {
    hovered.current = null;
    clearTimeout(openTimer.current);
    leave();
  }, [leave]);

  return { peek, close, keep, leave, hoverProps: { onMouseOver, onMouseLeave } };
}

interface DefinitionPeekProps {
  peek: Peek;
  /** Open a match: inside ARS when the file is in the review, else in the editor */
  onOpen: (path: string, line: number) => void;
  editorLabel: string;
  onEditor: (path: string, line: number) => void;
  onKeep: () => void;
  onLeave: () => void;
  onClose: () => void;
}

export function DefinitionPeek({ peek, onOpen, editorLabel, onEditor, onKeep, onLeave, onClose }: DefinitionPeekProps) {
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(true, onClose, ref);
  const { anchor, result, error, name } = peek;
  // Below the name when there is room, else above it, whichever is larger
  const below = window.innerHeight - anchor.bottom >= anchor.top;
  const style = {
    left: Math.max(12, Math.min(anchor.left, window.innerWidth - WIDTH - 12)),
    width: WIDTH,
    ...(below ? { top: anchor.bottom + 6 } : { bottom: window.innerHeight - anchor.top + 6 }),
    // Never past the window's edge: the rest scrolls
    maxHeight: Math.min(640, (below ? window.innerHeight - anchor.bottom : anchor.top) - 18),
  };
  const shown = result?.matches.length ?? 0;
  return (
    <div
      ref={ref}
      className="def-peek"
      style={style}
      role="dialog"
      aria-label={`Definition of ${name}`}
      onMouseEnter={onKeep}
      onMouseLeave={onLeave}
    >
      {!result && !error && <div className="def-peek-status faint">Looking for {name}…</div>}
      {error && <div className="def-peek-status">Could not look it up: {error}</div>}
      {result && shown === 0 && (
        <div className="def-peek-status faint">
          No definition of <span className="mono">{name}</span> in the project: it may come from a library.
        </div>
      )}
      {result?.matches.map((m) => {
        const lang = extToLang(m.path.split('.').pop()?.toLowerCase() ?? '');
        return (
          <section key={`${m.path}:${m.line}`} className="def-peek-match">
            <div className="def-peek-where">
              <button className="linklike mono" title="Open it here, or in your editor when it is not in the review" onClick={() => onOpen(m.path, m.line)}>
                {m.path}:{m.line}
              </button>
              <button className="btn-mini" onClick={() => onEditor(m.path, m.line)}>
                Open in {editorLabel}
              </button>
            </div>
            <pre className="diff def-peek-code">
              {m.doc.map((l, i) => (
                <div key={`d${i}`} className="def-peek-doc">
                  {l || ' '}
                </div>
              ))}
              {m.code.map((l, i) => (
                <div key={`c${i}`}>{l ? highlightLine(l, lang) : ' '}</div>
              ))}
            </pre>
            {m.more ? (
              <button className="linklike def-peek-more" onClick={() => onEditor(m.path, m.line)}>
                {m.more} more line{m.more === 1 ? '' : 's'}: open it in {editorLabel}
              </button>
            ) : null}
          </section>
        );
      })}
      {result && result.total > 1 && (
        <div className="def-peek-note faint">
          Found by name: {result.total} places look like a definition of {name}
          {result.total > shown ? `, the first ${shown} shown` : ''}.
        </div>
      )}
    </div>
  );
}
