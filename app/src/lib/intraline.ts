import type { DiffHunk } from '../types/domain';

// Word-level diff inside changed line pairs, GitHub-style: within a hunk,
// each run of deletions immediately followed by a run of additions is paired
// line-by-line, and the changed middle (common prefix/suffix trimmed) is
// highlighted on both sides. Pairs that share too little are left unmarked —
// the row background already says "this line changed".

export interface IntraRange {
  /** [start, end) character offsets into the line text */
  start: number;
  end: number;
}

const MAX_PAIR_CHARS = 1000;

function pairRanges(a: string, b: string): { del: IntraRange; add: IntraRange } | null {
  if (a === b) return null;
  const max = Math.max(a.length, b.length);
  if (max === 0 || max > MAX_PAIR_CHARS) return null;
  const min = Math.min(a.length, b.length);
  let prefix = 0;
  while (prefix < min && a[prefix] === b[prefix]) prefix++;
  let suffix = 0;
  while (suffix < min - prefix && a[a.length - 1 - suffix] === b[b.length - 1 - suffix]) suffix++;
  // Lines that share almost nothing are rewrites, not edits — highlighting
  // nearly the whole line is noise.
  if ((prefix + suffix) / max < 0.25) return null;
  return {
    del: { start: prefix, end: a.length - suffix },
    add: { start: prefix, end: b.length - suffix },
  };
}

/** Changed segments for a hunk, keyed by line index into hunk.lines */
export function changedRanges(hunk: DiffHunk): Map<number, IntraRange> {
  const out = new Map<number, IntraRange>();
  const lines = hunk.lines;
  let i = 0;
  while (i < lines.length) {
    if (lines[i].kind !== 'del') {
      i++;
      continue;
    }
    const delStart = i;
    while (i < lines.length && lines[i].kind === 'del') i++;
    const addStart = i;
    while (i < lines.length && lines[i].kind === 'add') i++;
    const pairs = Math.min(addStart - delStart, i - addStart);
    for (let k = 0; k < pairs; k++) {
      const ranges = pairRanges(lines[delStart + k].text, lines[addStart + k].text);
      if (!ranges) continue;
      if (ranges.del.start < ranges.del.end) out.set(delStart + k, ranges.del);
      if (ranges.add.start < ranges.add.end) out.set(addStart + k, ranges.add);
    }
  }
  return out;
}
