import { useCallback, useMemo, useState } from 'react';
import type { ReviewFile, ReviewHunk } from '../../types/review';

/** Noise (whitespace only, moved code) and generated files start folded */
const foldsByDefault = (file: ReviewFile, h: ReviewHunk) => Boolean(h.noise || file.generated);

/** Which of the file's hunks show as their header only. Folding or opening
 * one is kept per hunk (by content id) while the page is open. */
export function useFolds(file: ReviewFile | undefined) {
  const [choice, setChoice] = useState<ReadonlyMap<string, boolean>>(new Map());
  const choose = useCallback((id: string, folded: boolean) => setChoice((m) => new Map(m).set(id, folded)), []);
  const collapsed = useMemo(() => {
    const out = new Set<number>();
    file?.hunks.forEach((h, i) => {
      if (choice.get(h.id) ?? foldsByDefault(file, h)) out.add(i);
    });
    return out;
  }, [file, choice]);
  return {
    collapsed: collapsed as ReadonlySet<number>,
    toggle: (hi: number) => {
      const h = file?.hunks[hi];
      if (h) choose(h.id, !collapsed.has(hi));
    },
    /** Open a hunk, in any file */
    unfold: (id: string) => choose(id, false),
  };
}
