// The editors a file can open in, through their URL schemes. The desktop
// lets exactly these schemes through.

export type EditorId = 'vscode' | 'cursor' | 'windsurf';

export const EDITORS: Record<EditorId, { label: string; scheme: string }> = {
  vscode: { label: 'VS Code', scheme: 'vscode' },
  cursor: { label: 'Cursor', scheme: 'cursor' },
  windsurf: { label: 'Windsurf', scheme: 'windsurf' },
};

export function isEditorId(value: string): value is EditorId {
  return value in EDITORS;
}

/** A link that opens `path` (relative to `root`) at `line`, e.g.
 * "vscode://file/C:/repo/my%20notes.md:12". Every path segment is encoded,
 * so spaces and "#" survive; a drive letter stays as it is. */
export function editorLink(editor: EditorId, root: string, path: string, line?: number): string {
  const abs = `${root}/${path}`.replace(/\\/g, '/');
  const encoded = abs
    .split('/')
    .map((seg) => (/^[A-Za-z]:$/.test(seg) ? seg : encodeURIComponent(seg)))
    .join('/');
  return `${EDITORS[editor].scheme}://file/${encoded}${line ? `:${line}` : ''}`;
}
