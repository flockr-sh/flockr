// Server-side encoder for the mock: cell grid -> surface.frame / surface.delta,
// following section 10 (style table, runs, row diff, shift detection).
import type { Line, Run, Style, SurfaceDelta, SurfaceFrame } from '../src/protocol/types';
import type { Cell } from './terminal';

const MAX_STYLES = 4096;

export class SurfaceEncoder {
  private styles: Style[] = [[-1, -1, 0]];
  private styleIds = new Map<string, number>([['-1,-1,0', 0]]);
  private pendingStyles: Style[] = [];
  private lastRows: string[] | null = null;
  private lastCols = 0;
  seq = 0;

  constructor(private readonly allowShift = true) {}

  private styleId(s: Style): number {
    const key = `${s[0]},${s[1]},${s[2]}`;
    let id = this.styleIds.get(key);
    if (id === undefined) {
      id = this.styles.length;
      this.styles.push(s);
      this.pendingStyles.push(s);
      this.styleIds.set(key, id);
    }
    return id;
  }

  encodeRow(cells: Cell[]): Line {
    const runs: Run[] = [];
    let cur: [number, number, string] | null = null;
    let pendingSpaces = 0; // default-style spaces not yet committed
    for (let x = 0; x < cells.length; x++) {
      const c = cells[x];
      if (c.ch === '') continue; // wide continuation
      const ch = c.ch || ' ';
      const sid = this.styleId(c.style);
      const isWide = ch.length !== 1 || (x + 1 < cells.length && cells[x + 1].ch === '');
      if (isWide) {
        cur = null;
        pendingSpaces = 0;
        const w = x + 1 < cells.length && cells[x + 1].ch === '' ? 2 : 1;
        runs.push([x, sid, ch, w]);
        continue;
      }
      if (sid === 0 && ch === ' ') {
        if (cur && cur[1] === 0) pendingSpaces++;
        else cur = null;
        continue;
      }
      if (cur && cur[1] === sid && pendingSpaces <= 1 && cur[0] + cur[2].length + pendingSpaces === x) {
        cur[2] += ' '.repeat(pendingSpaces) + ch;
        pendingSpaces = 0;
        continue;
      }
      // A default-style gap of 1 space inside default text stays in the run;
      // longer gaps start a new run.
      pendingSpaces = 0;
      cur = [x, sid, ch];
      runs.push(cur);
    }
    return runs;
  }

  private takeStyles(): Style[] {
    const s = this.pendingStyles;
    this.pendingStyles = [];
    return s;
  }

  reset(): void {
    this.styles = [[-1, -1, 0]];
    this.styleIds = new Map([['-1,-1,0', 0]]);
    this.pendingStyles = [];
    this.lastRows = null;
  }

  /** Encode `cells` as a frame or a delta against the last encode. Returns null if nothing changed. */
  encode(
    sub: number,
    cells: Cell[][],
    cols: number,
    extra: { cursor: SurfaceFrame['cursor']; scroll: SurfaceFrame['scroll']; alt: boolean },
    forceFull = false,
  ): SurfaceFrame | SurfaceDelta | null {
    if (this.styles.length > MAX_STYLES) forceFull = true;
    const rows = cells.length;
    if (forceFull || !this.lastRows || this.lastRows.length !== rows || this.lastCols !== cols) {
      this.reset();
      const lines = cells.map((r) => this.encodeRow(r));
      this.takeStyles();
      this.lastRows = lines.map((l) => JSON.stringify(l));
      this.lastCols = cols;
      this.lastCursor = JSON.stringify(extra.cursor);
      this.seq++;
      return {
        t: 'surface.frame',
        sub,
        seq: this.seq,
        cols,
        rows,
        styles: this.styles.slice(),
        lines,
        cursor: extra.cursor,
        scroll: extra.scroll,
        alt: extra.alt,
      };
    }
    const lines = cells.map((r) => this.encodeRow(r));
    const next = lines.map((l) => JSON.stringify(l));
    const last = this.lastRows;
    const changedPlain = countDiff(last, next, 0);
    let shift = 0;
    if (this.allowShift) {
      for (let n = 1; n <= Math.floor(rows / 2); n++) {
        let match = 0;
        for (let y = 0; y < rows - n; y++) if (last[y + n] === next[y]) match++;
        if (match * 2 >= rows - n && countDiff(last, next, n) < changedPlain) {
          shift = n;
          break;
        }
      }
    }
    const shifted = last.slice();
    if (shift > 0) {
      for (let y = 0; y < rows - shift; y++) shifted[y] = last[y + shift];
      for (let y = rows - shift; y < rows; y++) shifted[y] = '[]';
    }
    const changed: [number, Line][] = [];
    for (let y = 0; y < rows; y++) if (shifted[y] !== next[y]) changed.push([y, lines[y]]);
    const styles_add = this.takeStyles();
    const cursorSame = JSON.stringify(extra.cursor) === this.lastCursor;
    if (changed.length === 0 && shift === 0 && styles_add.length === 0 && cursorSame) return null;
    this.lastRows = next;
    this.lastCursor = JSON.stringify(extra.cursor);
    const base = this.seq;
    this.seq++;
    const delta: SurfaceDelta = {
      t: 'surface.delta',
      sub,
      seq: this.seq,
      base,
      lines: changed,
      cursor: extra.cursor,
      scroll: extra.scroll,
      alt: extra.alt,
    };
    if (shift) delta.shift = shift;
    if (styles_add.length) delta.styles_add = styles_add;
    return delta;
  }

  private lastCursor = '';
}

function countDiff(last: string[], next: string[], n: number): number {
  let d = 0;
  for (let y = 0; y < next.length; y++) {
    const prev = y + n < last.length ? last[y + n] : '[]';
    if (prev !== next[y]) d++;
  }
  return d;
}
