// Cell grid state and the frame/delta reducer (section 10.3-10.5).
// Mutates in place for speed and reports what changed so the renderer only
// re-records the affected rows.
import type { Cursor, Line, Run, ScrollInfo, Style, SurfaceDelta, SurfaceFrame } from './types';

export interface Grid {
  seq: number;
  cols: number;
  rows: number;
  styles: Style[];
  links: string[];
  lines: Line[];
  cursor: Cursor | null;
  scroll: ScrollInfo | null;
  alt: boolean;
}

export interface GridChange {
  /** Everything changed (new frame): redraw all rows. */
  full: boolean;
  /** Whole-grid shift applied before `rows` (positive = up). */
  shift: number;
  /** Rows replaced by this update, after the shift. */
  rows: number[];
  cursorChanged: boolean;
  scrollChanged: boolean;
  sizeChanged: boolean;
}

export type ApplyResult = { ok: true; change: GridChange } | { ok: false; reason: 'base_mismatch' | 'no_grid' };

export const DEFAULT_STYLE: Style = [-1, -1, 0];

export function emptyGrid(): Grid {
  return {
    seq: 0,
    cols: 0,
    rows: 0,
    styles: [DEFAULT_STYLE],
    links: [],
    lines: [],
    cursor: null,
    scroll: null,
    alt: false,
  };
}

export function applyFrame(grid: Grid, frame: SurfaceFrame): GridChange {
  const sizeChanged = grid.cols !== frame.cols || grid.rows !== frame.rows;
  grid.seq = frame.seq;
  grid.cols = frame.cols;
  grid.rows = frame.rows;
  grid.styles = frame.styles.length > 0 ? frame.styles.slice() : [DEFAULT_STYLE];
  grid.links = frame.links ? frame.links.slice() : [];
  const lines: Line[] = new Array(frame.rows);
  for (let y = 0; y < frame.rows; y++) lines[y] = frame.lines[y] ?? [];
  grid.lines = lines;
  grid.cursor = frame.cursor ?? null;
  grid.scroll = frame.scroll ?? null;
  grid.alt = !!frame.alt;
  return { full: true, shift: 0, rows: [], cursorChanged: true, scrollChanged: true, sizeChanged };
}

/**
 * Move array entries by `n` (positive = toward index 0, i.e. scroll up) and
 * fill the vacated slots with `fill()`. Shared by the grid and the renderer's
 * per-row picture cache so both shift identically.
 */
export function shiftRows<T>(arr: T[], n: number, fill: () => T): void {
  const len = arr.length;
  if (n === 0 || len === 0) return;
  if (Math.abs(n) >= len) {
    for (let i = 0; i < len; i++) arr[i] = fill();
    return;
  }
  if (n > 0) {
    for (let y = 0; y < len - n; y++) arr[y] = arr[y + n];
    for (let y = len - n; y < len; y++) arr[y] = fill();
  } else {
    const m = -n;
    for (let y = len - 1; y >= m; y--) arr[y] = arr[y - m];
    for (let y = 0; y < m; y++) arr[y] = fill();
  }
}

export function applyDelta(grid: Grid, delta: SurfaceDelta): ApplyResult {
  if (grid.seq === 0) return { ok: false, reason: 'no_grid' };
  if (delta.base !== grid.seq) return { ok: false, reason: 'base_mismatch' };
  if (delta.styles_add) for (const s of delta.styles_add) grid.styles.push(s);
  if (delta.links_add) for (const l of delta.links_add) grid.links.push(l);
  let shift = delta.shift ?? 0;
  if (shift !== 0 && Math.abs(shift) < grid.rows) {
    shiftRows(grid.lines, shift, () => [] as Line);
  } else {
    shift = 0;
  }
  const rows: number[] = [];
  if (delta.lines) {
    for (const [y, runs] of delta.lines) {
      if (y >= 0 && y < grid.rows) {
        grid.lines[y] = runs;
        rows.push(y);
      }
    }
  }
  let cursorChanged = false;
  if (delta.cursor !== undefined) {
    grid.cursor = delta.cursor;
    cursorChanged = true;
  }
  let scrollChanged = false;
  if (delta.scroll !== undefined) {
    grid.scroll = delta.scroll;
    scrollChanged = true;
  }
  if (delta.alt !== undefined) grid.alt = delta.alt;
  grid.seq = delta.seq;
  return { ok: true, change: { full: false, shift, rows, cursorChanged, scrollChanged, sizeChanged: false } };
}

/** Columns a run covers. */
export function runWidth(run: Run): number {
  return run.length === 4 ? run[3] : run[2].length;
}

/** Plain text of one line, blanks filled with spaces (for tests and copy). */
export function lineText(line: Line, cols: number): string {
  const cells: string[] = new Array(cols).fill(' ');
  for (const run of line) {
    const x = run[0];
    if (run.length === 4) {
      if (x < cols) cells[x] = run[2];
      for (let i = 1; i < run[3] && x + i < cols; i++) cells[x + i] = '';
    } else {
      const text = run[2];
      for (let i = 0; i < text.length && x + i < cols; i++) cells[x + i] = text[i];
    }
  }
  return cells.join('').replace(/\s+$/, '');
}

export function gridText(grid: Grid): string[] {
  return grid.lines.map((l) => lineText(l, grid.cols));
}

// --- Flags (section 10.3) ---

export const Flag = {
  bold: 1,
  dim: 2,
  italic: 4,
  underline: 8,
  slowBlink: 16,
  rapidBlink: 32,
  reverse: 64,
  hidden: 128,
  strikethrough: 256,
} as const;

export function underlineStyle(flags: number): number {
  return (flags >> 9) & 0xf;
}

// --- Colors ---

export const TRUECOLOR_BASE = 16777216;

/** xterm default 16 colors. */
export const XTERM_16: number[] = [
  0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5, 0x7f7f7f, 0xff0000, 0x00ff00,
  0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
];

export const DEFAULT_FG = 0xd8dee9;
export const DEFAULT_BG = 0x15161e;

export interface ResolvedTheme {
  fg: number;
  bg: number;
  palette: number[]; // 256 entries, 0xRRGGBB
}

function parseHex(value: string | null | undefined): number | null {
  if (!value) return null;
  const m = /^#([0-9a-fA-F]{6})$/.exec(value);
  return m ? parseInt(m[1], 16) : null;
}

export function xtermColor(index: number): number {
  if (index < 16) return XTERM_16[index];
  if (index < 232) {
    const i = index - 16;
    const steps = [0, 95, 135, 175, 215, 255];
    const r = steps[Math.floor(i / 36)];
    const g = steps[Math.floor(i / 6) % 6];
    const b = steps[i % 6];
    return (r << 16) | (g << 8) | b;
  }
  const v = 8 + (index - 232) * 10;
  return (v << 16) | (v << 8) | v;
}

export function resolveTheme(theme?: {
  fg?: string | null;
  bg?: string | null;
  palette?: (string | null)[];
}): ResolvedTheme {
  const palette: number[] = new Array(256);
  for (let i = 0; i < 256; i++) palette[i] = xtermColor(i);
  if (theme?.palette) {
    for (let i = 0; i < 16 && i < theme.palette.length; i++) {
      const c = parseHex(theme.palette[i]);
      if (c !== null) palette[i] = c;
    }
  }
  return {
    fg: parseHex(theme?.fg) ?? DEFAULT_FG,
    bg: parseHex(theme?.bg) ?? DEFAULT_BG,
    palette,
  };
}

/** Resolve a wire color to 0xRRGGBB. `isFg` picks the default. */
export function resolveColor(value: number, theme: ResolvedTheme, isFg: boolean): number {
  if (value < 0) return isFg ? theme.fg : theme.bg;
  if (value >= TRUECOLOR_BASE) return (value - TRUECOLOR_BASE) & 0xffffff;
  if (value < 256) return theme.palette[value];
  return isFg ? theme.fg : theme.bg;
}

export interface ResolvedStyle {
  fg: number;
  bg: number;
  /** Background differs from the theme background and must be painted. */
  paintBg: boolean;
  bold: boolean;
  dim: boolean;
  italic: boolean;
  underline: number; // 0 none, else underline style (1 single ...)
  strike: boolean;
  hidden: boolean;
  link: number; // -1 none
}

export function resolveStyle(style: Style | undefined, theme: ResolvedTheme): ResolvedStyle {
  const s = style ?? DEFAULT_STYLE;
  const flags = s[2];
  let fg = resolveColor(s[0], theme, true);
  let bg = resolveColor(s[1], theme, false);
  // Bold with a basic color brightens it, as most terminals do.
  if (flags & Flag.bold && s[0] >= 0 && s[0] < 8) fg = theme.palette[s[0] + 8];
  let paintBg = s[1] !== -1;
  if (flags & Flag.reverse) {
    const t = fg;
    fg = bg;
    bg = t;
    paintBg = true;
  }
  const ul = flags & Flag.underline ? Math.max(1, underlineStyle(flags)) : 0;
  return {
    fg,
    bg,
    paintBg,
    bold: !!(flags & Flag.bold),
    dim: !!(flags & Flag.dim),
    italic: !!(flags & Flag.italic),
    underline: ul,
    strike: !!(flags & Flag.strikethrough),
    hidden: !!(flags & Flag.hidden),
    link: s.length === 4 ? s[3] : -1,
  };
}

const URL_RE = /\b(?:https?:\/\/|file:\/\/)[^\s<>"'`]+/g;

/**
 * The link under a cell: an OSC 8 hyperlink from the style table, else a
 * plain-text URL in the line that covers the column.
 */
export function linkAt(grid: Grid, col: number, row: number): string | null {
  const line = grid.lines[row];
  if (!line || col < 0 || col >= grid.cols) return null;
  for (const run of line) {
    const x = run[0];
    if (col < x || col >= x + runWidth(run)) continue;
    const style = grid.styles[run[1]];
    if (style && style.length === 4) return grid.links[style[3]] ?? null;
    break;
  }
  const text = lineText(line, grid.cols);
  for (const m of text.matchAll(URL_RE)) {
    const start = m.index ?? 0;
    // Trailing punctuation is usually prose, not part of the URL.
    const url = m[0].replace(/[.,;:!?)\]]+$/, '');
    if (col >= start && col < start + url.length) return url;
  }
  return null;
}
