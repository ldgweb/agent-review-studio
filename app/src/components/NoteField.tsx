import { useCallback, useEffect, useRef, useState } from 'react';

/** Autosave delay: each save writes the project's review */
const NOTE_SAVE_DELAY_MS = 500;

/** A textarea that keeps its text locally and saves it after a pause, on
 * blur and on unmount, never once per keystroke. It picks up the stored
 * value whenever no local edit is waiting. */
export function NoteField({
  value,
  onSave,
  focusToken = 0,
  ...rest
}: {
  value: string;
  onSave: (text: string) => void;
  /** A new value focuses the field. The value it mounts with does not, so a
   * field that remounts (another file) never steals the focus. */
  focusToken?: number;
  className: string;
  rows: number;
  placeholder?: string;
  'aria-label': string;
}) {
  const [text, setText] = useState(value);
  const dirty = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const textRef = useRef(text);
  textRef.current = text;
  const onSaveRef = useRef(onSave);
  onSaveRef.current = onSave;
  const ref = useRef<HTMLTextAreaElement>(null);
  const seenToken = useRef(focusToken);

  const flush = useCallback(() => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    if (!dirty.current) return;
    dirty.current = false;
    onSaveRef.current(textRef.current);
  }, []);

  useEffect(() => {
    if (!dirty.current) setText(value);
  }, [value]);

  useEffect(() => flush, [flush]);

  useEffect(() => {
    if (focusToken === seenToken.current) return;
    seenToken.current = focusToken;
    ref.current?.focus();
  }, [focusToken]);

  return (
    <textarea
      {...rest}
      ref={ref}
      value={text}
      onChange={(e) => {
        setText(e.target.value);
        dirty.current = true;
        if (timer.current) clearTimeout(timer.current);
        timer.current = setTimeout(flush, NOTE_SAVE_DELAY_MS);
      }}
      onBlur={flush}
    />
  );
}
