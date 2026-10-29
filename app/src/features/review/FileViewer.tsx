import { useEffect, useMemo, useRef, useState } from 'react';
import type { ReviewFile } from '../../types/review';
import { LINE_HEIGHT } from '../../components/DiffView';
import { IconX } from '../../components/icons';
import { errorMessage } from '../../lib/api';
import { EDITORS, type EditorId } from '../../lib/editors';
import { extToLang, highlightFile, highlightLine, renderHast } from '../../lib/highlight';
import { useDismiss } from '../../state/useDismiss';

// The whole file as it is now, read-only, colored like an editor and open in
// an instant: to see the code around a change without leaving the review.
// Changed lines are marked in the gutter. Your own editor stays one click
// away, for when you want to edit.

/** Lines rendered per block; offscreen blocks skip layout */
const CHUNK = 200;

interface FileViewerProps {
  file: ReviewFile;
  /** The file as it is now (HEAD's for a deleted file); throws the reason */
  load: () => Promise<string[]>;
  /** Code font size (px), the diff's */
  fontSize: number;
  /** The line to show first; the first change otherwise */
  focusLine?: number;
  editor: EditorId;
  onEditor: (id: EditorId) => void;
  onOpenInEditor?: (line?: number) => void;
  onClose: () => void;
}

export function FileViewer({ file, load, fontSize, focusLine, editor, onEditor, onOpenInEditor, onClose }: FileViewerProps) {
  const [lines, setLines] = useState<string[] | null>(null);
  const [failure, setFailure] = useState('');
  const [menu, setMenu] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLSpanElement>(null);
  const lang = useMemo(() => extToLang(file.language), [file.language]);
  const deleted = file.changeType === 'deleted';
  useDismiss(true, onClose);
  useDismiss(menu, () => setMenu(false), menuRef);

  useEffect(() => {
    let live = true;
    load().then(
      (l) => {
        if (live) setLines(l[l.length - 1] === '' ? l.slice(0, -1) : l);
      },
      (e: unknown) => {
        if (live) setFailure(errorMessage(e));
      },
    );
    return () => {
      live = false;
    };
    // Loaded once per opening: the owner remounts the viewer when the file changes
  }, []);

  // Where the changes are: added or rewritten lines, and the spots where
  // lines were removed (the line right after them)
  const { added, removedAt, firstChange } = useMemo(() => {
    const added = new Set<number>();
    const removedAt = new Set<number>();
    for (const h of file.hunks) {
      h.lines.forEach((l, i) => {
        if (l.kind === 'add' && l.newLine != null) added.add(l.newLine);
        if (l.kind === 'del' && h.lines[i + 1]?.kind !== 'del') {
          const after = h.lines.slice(i + 1).find((x) => x.newLine != null);
          if (after?.newLine != null) removedAt.add(after.newLine);
        }
      });
    }
    const all = [...added, ...removedAt];
    return { added, removedAt, firstChange: all.length ? Math.min(...all) : undefined };
  }, [file]);

  const tokens = useMemo(() => (lines ? highlightFile(lines, lang) : null), [lines, lang]);
  const target = focusLine ?? firstChange;

  useEffect(() => {
    if (!lines || !target) return;
    requestAnimationFrame(() =>
      bodyRef.current?.querySelector(`[data-n="${target}"]`)?.scrollIntoView({ block: 'center' }),
    );
  }, [lines, target]);

  const chunks = useMemo(() => {
    if (!lines) return [];
    const out: number[] = [];
    for (let i = 0; i < lines.length; i += CHUNK) out.push(i);
    return out;
  }, [lines]);

  return (
    <div className="viewer" role="dialog" aria-label={`${file.path}, whole file`}>
      <div className="viewer-head">
        <span className="file-head-path">
          <strong>{file.path}</strong>
        </span>
        <span className="file-head-kind">{deleted ? 'deleted · as in HEAD' : 'whole file, as it is now'}</span>
        {lines && <span className="faint viewer-count">{lines.length} lines</span>}
        <span style={{ flex: 1 }} />
        {onOpenInEditor && !deleted && (
          <span className="split-btn menu-anchor" ref={menuRef}>
            <button className="btn-tool" title="Edit it yourself, in your editor" onClick={() => onOpenInEditor(target)}>
              Open in {EDITORS[editor].label}
            </button>
            <button className="btn-tool split-btn-more" aria-label="Choose the editor" aria-expanded={menu} onClick={() => setMenu(!menu)}>
              ▾
            </button>
            {menu && (
              <div className="menu">
                {(Object.keys(EDITORS) as EditorId[]).map((id) => (
                  <button
                    key={id}
                    aria-pressed={editor === id}
                    onClick={() => {
                      onEditor(id);
                      setMenu(false);
                    }}
                  >
                    <span className="menu-check">{editor === id ? '✓' : ''}</span> {EDITORS[id].label}
                  </button>
                ))}
              </div>
            )}
          </span>
        )}
        <button className="btn-tool" title="Back to the changes (Esc)" onClick={onClose}>
          <IconX size={14} /> Close
        </button>
      </div>
      <div className="viewer-body diff wrap" ref={bodyRef} style={{ fontSize }}>
        {failure && (
          <div className="diff-empty" role="alert">
            <strong>This file could not be read.</strong> {failure}
          </div>
        )}
        {lines?.length === 0 && <div className="diff-empty">The file is empty.</div>}
        {!lines && !failure && (
          <div className="diff-empty">
            <span className="spinner" aria-hidden="true" /> Opening…
          </div>
        )}
        {lines &&
          chunks.map((start) => (
            <div
              key={start}
              className="viewer-chunk"
              style={{ containIntrinsicHeight: `auto ${Math.min(CHUNK, lines.length - start) * Math.round(fontSize * LINE_HEIGHT)}px` }}
            >
              {lines.slice(start, start + CHUNK).map((text, k) => {
                const n = start + k + 1;
                const mark = deleted ? '' : added.has(n) ? ' changed' : removedAt.has(n) ? ' removed-before' : '';
                return (
                  <div key={n} className={`viewer-line${mark}`} data-n={n}>
                    <span className="viewer-ln">{n}</span>
                    <span className="code">{tokens?.[n - 1] ? renderHast(tokens[n - 1]) : highlightLine(text, lang)}</span>
                  </div>
                );
              })}
            </div>
          ))}
      </div>
    </div>
  );
}
