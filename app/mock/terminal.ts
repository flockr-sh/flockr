// A tiny fake terminal for the mock gateway: a cell grid with a cursor,
// line feeds, scrolling and a few canned content generators.
import type { Style } from '../src/protocol/types';

export interface Cell {
  ch: string; // one grapheme; '' for the continuation of a wide char
  style: Style; // [fg, bg, flags]
}

export const BLANK_STYLE: Style = [-1, -1, 0];

export function blankCell(): Cell {
  return { ch: ' ', style: BLANK_STYLE };
}

const WIDE_RE = /[ᄀ-ᅟ⺀-〾ぁ-㏿㐀-䶿一-鿿ꀀ-꓏가-힣豈-﫿︰-﹏＀-｠￠-￦]|[\u{1f300}-\u{1faff}]/u;

export function graphemeWidth(g: string): number {
  if (g.length === 1 && g.charCodeAt(0) < 0x1100) return 1;
  return WIDE_RE.test(g) ? 2 : 1;
}

export function splitGraphemes(text: string): string[] {
  // Good enough for the mock: code points, with combining marks joined.
  const out: string[] = [];
  for (const ch of text) {
    if (ch.charCodeAt(0) >= 0x300 && /\p{M}/u.test(ch) && out.length > 0) out[out.length - 1] += ch;
    else out.push(ch);
  }
  return out;
}

export class FakeTerminal {
  cells: Cell[][];
  cursorX = 0;
  cursorY = 0;
  scrollback: string[] = [];
  alt = false;
  /** Bumped on every change; the gateway diffs when it moves. */
  version = 0;

  constructor(
    public cols: number,
    public rows: number,
  ) {
    this.cells = Array.from({ length: rows }, () => this.blankRow());
  }

  private blankRow(): Cell[] {
    return Array.from({ length: this.cols }, blankCell);
  }

  resize(cols: number, rows: number): void {
    const old = this.cells;
    this.cols = cols;
    this.rows = rows;
    this.cells = Array.from({ length: rows }, (_, y) => {
      const src = old[old.length - rows + y];
      const row = this.blankRow();
      if (src) for (let x = 0; x < Math.min(cols, src.length); x++) row[x] = src[x];
      return row;
    });
    this.cursorX = Math.min(this.cursorX, cols - 1);
    this.cursorY = Math.min(Math.max(0, this.cursorY), rows - 1);
    this.version++;
  }

  rowText(y: number): string {
    return this.cells[y]
      .map((c) => c.ch)
      .join('')
      .replace(/\s+$/, '');
  }

  lineFeed(): void {
    if (this.cursorY < this.rows - 1) {
      this.cursorY++;
    } else {
      this.scrollback.push(this.rowText(0));
      if (this.scrollback.length > 5000) this.scrollback.shift();
      this.cells.shift();
      this.cells.push(this.blankRow());
    }
    this.cursorX = 0;
    this.version++;
  }

  write(text: string, style: Style = BLANK_STYLE): void {
    for (const g of splitGraphemes(text)) {
      if (g === '\n') {
        this.lineFeed();
        continue;
      }
      if (g === '\r') {
        this.cursorX = 0;
        continue;
      }
      const w = graphemeWidth(g);
      if (this.cursorX + w > this.cols) this.lineFeed();
      this.cells[this.cursorY][this.cursorX] = { ch: g, style };
      if (w === 2 && this.cursorX + 1 < this.cols) this.cells[this.cursorY][this.cursorX + 1] = { ch: '', style };
      this.cursorX += w;
    }
    this.version++;
  }

  writeLine(text: string, style: Style = BLANK_STYLE): void {
    this.write(text, style);
    this.lineFeed();
  }

  backspace(): void {
    if (this.cursorX > 0) {
      this.cursorX--;
      this.cells[this.cursorY][this.cursorX] = blankCell();
      this.version++;
    }
  }

  clear(): void {
    this.cells = Array.from({ length: this.rows }, () => this.blankRow());
    this.cursorX = 0;
    this.cursorY = 0;
    this.version++;
  }

  /** Put text at a position without moving the cursor (TUI-style). */
  put(x: number, y: number, text: string, style: Style = BLANK_STYLE): void {
    if (y < 0 || y >= this.rows) return;
    let cx = x;
    for (const g of splitGraphemes(text)) {
      if (cx >= this.cols) break;
      const w = graphemeWidth(g);
      this.cells[y][cx] = { ch: g, style };
      if (w === 2 && cx + 1 < this.cols) this.cells[y][cx + 1] = { ch: '', style };
      cx += w;
    }
    this.version++;
  }
}
