import type { DiffHunk } from '../types/domain';

// Word-level changes inside changed lines, the way the best viewers do it
// (GitHub, VS Code, JetBrains): each run of removed lines is aligned with
// the run of added lines after it by similarity, not by position, so an
// inserted line does not shift every pair below it; within an aligned pair,
// a token diff (words, spaces, single punctuation) marks every changed
// segment, not just one span from the first to the last difference. Pairs
// that share too little are rewrites and stay unmarked: the row color
// already says the line changed.

export interface IntraRange {
  /** [start, end) character offsets into the line text */
  start: number;
  end: number;
}

const MAX_PAIR_CHARS = 1000;
/** Token LCS table bound (tokens × tokens) */
const MAX_LCS_CELLS = 160_000;
/** Run sizes aligned by similarity; bigger runs pair by position */
const MAX_ALIGN = 60;
/** Below this similarity two lines are a rewrite, not an edit */
const MIN_SIMILARITY = 0.35;

function tokenize(text: string): string[] {
  return text.match(/[\p{L}\p{N}_]+|\s+|[^\p{L}\p{N}_\s]/gu) ?? [];
}

/** Character bigram overlap (Dice), trimmed: cheap and good at "same line, edited" */
export function similarity(a: string, b: string): number {
  const x = a.trim();
  const y = b.trim();
  if (x === y) return 1;
  if (x.length < 2 || y.length < 2) return 0;
  const grams = new Map<string, number>();
  for (let i = 0; i < x.length - 1; i++) {
    const g = x.slice(i, i + 2);
    grams.set(g, (grams.get(g) ?? 0) + 1);
  }
  let shared = 0;
  for (let i = 0; i < y.length - 1; i++) {
    const g = y.slice(i, i + 2);
    const n = grams.get(g) ?? 0;
    if (n > 0) {
      shared++;
      grams.set(g, n - 1);
    }
  }
  return (2 * shared) / (x.length + y.length - 2);
}

export interface RunRow {
  del: number | null;
  add: number | null;
  /** The two lines are the same line, edited (worth a word diff) */
  match: boolean;
}

/** Rows for a run of removed lines followed by added ones, in display
 * order: the pairing that maximizes total similarity without crossing;
 * between matched pairs, the leftovers share rows by position. */
export function alignRun(dels: string[], adds: string[]): RunRow[] {
  const n = dels.length;
  const m = adds.length;
  const rows: RunRow[] = [];
  let gapDels: number[] = [];
  let gapAdds: number[] = [];
  const flush = () => {
    for (let k = 0; k < Math.max(gapDels.length, gapAdds.length); k++) {
      rows.push({ del: gapDels[k] ?? null, add: gapAdds[k] ?? null, match: false });
    }
    gapDels = [];
    gapAdds = [];
  };
  for (const [d, a] of matchLines(dels, adds, n, m)) {
    if (d !== null && a !== null) {
      flush();
      rows.push({ del: d, add: a, match: true });
    } else if (d !== null) gapDels.push(d);
    else if (a !== null) gapAdds.push(a);
  }
  flush();
  return rows;
}

function matchLines(dels: string[], adds: string[], n: number, m: number): Array<[number | null, number | null]> {
  if (n === 0 || m === 0 || n > MAX_ALIGN || m > MAX_ALIGN) {
    const out: Array<[number | null, number | null]> = [];
    for (let k = 0; k < Math.max(n, m); k++) out.push([k < n ? k : null, k < m ? k : null]);
    return out;
  }
  const sim = dels.map((d) => adds.map((a) => similarity(d, a)));
  // best[i][j]: the best score aligning dels[i..] with adds[j..]
  const best = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      const pair = sim[i][j] >= MIN_SIMILARITY ? sim[i][j] + best[i + 1][j + 1] : -1;
      best[i][j] = Math.max(pair, best[i + 1][j], best[i][j + 1]);
    }
  }
  const out: Array<[number | null, number | null]> = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (sim[i][j] >= MIN_SIMILARITY && best[i][j] === sim[i][j] + best[i + 1][j + 1]) {
      out.push([i++, j++]);
    } else if (best[i][j] === best[i + 1][j]) {
      out.push([i++, null]);
    } else {
      out.push([null, j++]);
    }
  }
  while (i < n) out.push([i++, null]);
  while (j < m) out.push([null, j++]);
  return out;
}

/** Changed segments of a line pair, or null for a rewrite. */
export function pairRanges(a: string, b: string): { del: IntraRange[]; add: IntraRange[] } | null {
  if (a === b) return null;
  const max = Math.max(a.length, b.length);
  if (max === 0 || max > MAX_PAIR_CHARS) return null;
  const ta = tokenize(a);
  const tb = tokenize(b);
  // Common token prefix and suffix first: keeps the table small
  let pre = 0;
  while (pre < ta.length && pre < tb.length && ta[pre] === tb[pre]) pre++;
  let suf = 0;
  while (suf < ta.length - pre && suf < tb.length - pre && ta[ta.length - 1 - suf] === tb[tb.length - 1 - suf]) suf++;
  const ma = ta.slice(pre, ta.length - suf);
  const mb = tb.slice(pre, tb.length - suf);
  const keepA = new Array<boolean>(ma.length).fill(false);
  const keepB = new Array<boolean>(mb.length).fill(false);
  if (ma.length && mb.length && ma.length * mb.length <= MAX_LCS_CELLS) {
    const w = mb.length + 1;
    const lcs = new Uint16Array((ma.length + 1) * w);
    for (let i = ma.length - 1; i >= 0; i--) {
      for (let j = mb.length - 1; j >= 0; j--) {
        lcs[i * w + j] = ma[i] === mb[j] ? lcs[(i + 1) * w + j + 1] + 1 : Math.max(lcs[(i + 1) * w + j], lcs[i * w + j + 1]);
      }
    }
    for (let i = 0, j = 0; i < ma.length && j < mb.length; ) {
      if (ma[i] === mb[j]) {
        keepA[i++] = true;
        keepB[j++] = true;
      } else if (lcs[(i + 1) * w + j] >= lcs[i * w + j + 1]) i++;
      else j++;
    }
  }
  const ranges = (tokens: string[], middle: string[], keep: boolean[]): { ranges: IntraRange[]; same: number } => {
    let pos = tokens.slice(0, pre).join('').length;
    let same = pos + tokens.slice(tokens.length - suf).join('').length;
    const out: IntraRange[] = [];
    middle.forEach((tok, k) => {
      const end = pos + tok.length;
      if (keep[k]) same += tok.length;
      // a space between two changed tokens joins them into one segment
      const bridge = keep[k] && /^\s+$/.test(tok) && k > 0 && !keep[k - 1] && k + 1 < middle.length && !keep[k + 1];
      if (!keep[k] || bridge) {
        const last = out[out.length - 1];
        if (last && last.end === pos) last.end = end;
        else out.push({ start: pos, end });
      }
      pos = end;
    });
    return { ranges: out, same };
  };
  const del = ranges(ta, ma, keepA);
  const add = ranges(tb, mb, keepB);
  if ((del.same + add.same) / (a.length + b.length) < MIN_SIMILARITY) return null;
  return { del: del.ranges, add: add.ranges };
}

/** Changed segments for a hunk, keyed by line index into hunk.lines */
export function changedRanges(hunk: DiffHunk): Map<number, IntraRange[]> {
  const out = new Map<number, IntraRange[]>();
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
    const dels = lines.slice(delStart, addStart).map((l) => l.text);
    const adds = lines.slice(addStart, i).map((l) => l.text);
    for (const { del: d, add: a, match } of alignRun(dels, adds)) {
      if (!match || d === null || a === null) continue;
      const r = pairRanges(dels[d], adds[a]);
      if (!r) continue;
      if (r.del.length) out.set(delStart + d, r.del);
      if (r.add.length) out.set(addStart + a, r.add);
    }
  }
  return out;
}
