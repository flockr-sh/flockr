// Skia renderer for a cell grid. One SkPicture per row (re-recorded only when
// the row changes); a cheap composite picture places rows, cursor and zoom.
import { matchFont, PaintStyle, Skia, SkFont, SkPaint, SkPicture } from '@shopify/react-native-skia';
import { Platform } from 'react-native';
import { Grid, GridChange, Line, ResolvedStyle, ResolvedTheme, resolveStyle, shiftRows } from '../protocol';

export interface FontSet {
  regular: SkFont;
  bold: SkFont;
  italic: SkFont;
  boldItalic: SkFont;
  /** Wide-coverage fallback (JuliaMono) for symbols the main font lacks. */
  symbols: SkFont;
  /** Nerd Font icons (private use area) that shell prompts print. */
  nerd: SkFont;
}

export interface Metrics {
  cellW: number;
  cellH: number;
  baseline: number;
}

export function measure(font: SkFont): Metrics {
  const ids = font.getGlyphIDs('M');
  const cellW = font.getGlyphWidths(ids)[0] || font.getSize() * 0.6;
  const m = font.getMetrics();
  const ascent = Math.abs(m.ascent);
  const descent = Math.abs(m.descent);
  const cellH = Math.ceil(ascent + descent + (m.leading || 0));
  return { cellW, cellH, baseline: Math.round(ascent + (m.leading || 0) / 2) };
}

const hex = (c: number) => `#${c.toString(16).padStart(6, '0')}`;

const FALLBACK_FAMILIES = Platform.select({
  ios: ['Menlo', 'Apple Color Emoji', 'PingFang SC', 'Apple SD Gothic Neo', 'Apple Symbols'],
  android: ['monospace', 'sans-serif', 'Noto Color Emoji'],
  default: ['monospace'],
}) as string[];

// Characters outside Latin-1/Latin Extended may be missing from the bundled
// font; only those runs pay for a glyph lookup.
const MAYBE_MISSING = /[^\u0000-ɏ]/;

export class TerminalRenderer {
  readonly metrics: Metrics;
  rowPics: (SkPicture | null)[] = [];
  private styleCache: (ResolvedStyle | undefined)[] = [];
  private paints = new Map<string, SkPaint>();
  private fallbackFonts: SkFont[] | null = null;
  private glyphFont = new Map<string, SkFont | null>();
  private theme: ResolvedTheme | null = null;
  private cols = 0;

  constructor(private readonly fonts: FontSet) {
    this.metrics = measure(fonts.regular);
  }

  setTheme(theme: ResolvedTheme): void {
    if (this.theme !== theme) {
      this.theme = theme;
      this.styleCache = [];
      this.rowPics = this.rowPics.map(() => null);
    }
  }

  private paint(color: number, alpha = 1, stroke = 0): SkPaint {
    const key = `${color}:${alpha}:${stroke}`;
    let p = this.paints.get(key);
    if (!p) {
      p = Skia.Paint();
      p.setColor(Skia.Color(hex(color)));
      if (alpha !== 1) p.setAlphaf(alpha);
      p.setAntiAlias(true);
      if (stroke > 0) {
        p.setStyle(PaintStyle.Stroke);
        p.setStrokeWidth(stroke);
      }
      this.paints.set(key, p);
    }
    return p;
  }

  private style(grid: Grid, id: number): ResolvedStyle {
    let s = this.styleCache[id];
    if (!s && this.theme) {
      s = resolveStyle(grid.styles[id], this.theme);
      this.styleCache[id] = s;
    }
    return s as ResolvedStyle;
  }

  private fontFor(s: ResolvedStyle): SkFont {
    if (s.bold && s.italic) return this.fonts.boldItalic;
    if (s.bold) return this.fonts.bold;
    if (s.italic) return this.fonts.italic;
    return this.fonts.regular;
  }

  private fallbackFor(g: string, base: SkFont): SkFont | null {
    const cached = this.glyphFont.get(g);
    if (cached !== undefined) return cached;
    if (!this.fallbackFonts) {
      this.fallbackFonts = [this.fonts.symbols, this.fonts.nerd];
      for (const family of FALLBACK_FAMILIES) {
        try {
          this.fallbackFonts.push(matchFont({ fontFamily: family, fontSize: base.getSize() }));
        } catch {
          // family not on this device
        }
      }
    }
    let found: SkFont | null = null;
    for (const f of this.fallbackFonts) {
      if (!f.getGlyphIDs(g).includes(0)) {
        found = f;
        break;
      }
    }
    this.glyphFont.set(g, found);
    return found;
  }

  /** Apply a grid change: shift and re-record changed rows. Returns ms spent. */
  apply(change: GridChange, grid: Grid): number {
    const t0 = performance.now();
    if (change.full) {
      // New style table: style ids may mean something else now.
      this.styleCache = [];
      this.cols = grid.cols;
      this.rowPics = new Array(grid.rows);
      for (let y = 0; y < grid.rows; y++) this.rowPics[y] = this.recordRow(grid, grid.lines[y]);
    } else {
      if (change.shift) shiftRows(this.rowPics, change.shift, () => null);
      for (const y of change.rows) this.rowPics[y] = this.recordRow(grid, grid.lines[y]);
    }
    return performance.now() - t0;
  }

  private recordRow(grid: Grid, line: Line): SkPicture | null {
    if (!line || line.length === 0) return null;
    const { cellW, cellH, baseline } = this.metrics;
    const rec = Skia.PictureRecorder();
    const canvas = rec.beginRecording(Skia.XYWHRect(0, 0, this.cols * cellW, cellH));
    for (const run of line) {
      const x = run[0];
      const s = this.style(grid, run[1]);
      if (!s) continue;
      const text = run[2];
      const w = run.length === 4 ? run[3] : text.length;
      const px = x * cellW;
      const width = w * cellW;
      if (s.paintBg) canvas.drawRect(Skia.XYWHRect(px, 0, width, cellH), this.paint(s.bg));
      if (s.hidden) continue;
      const fg = this.paint(s.fg, s.dim ? 0.55 : 1);
      const font = this.fontFor(s);
      if (run.length === 4) {
        const f = font.getGlyphIDs(text).includes(0) ? (this.fallbackFor(text, font) ?? font) : font;
        canvas.drawText(text, px, baseline, fg, f);
      } else if (text.trim().length > 0) {
        if (MAYBE_MISSING.test(text) && font.getGlyphIDs(text).includes(0)) {
          // Draw per cell so fallback glyphs land on the grid.
          for (let i = 0; i < text.length; i++) {
            const ch = text[i];
            const f = font.getGlyphIDs(ch).includes(0) ? (this.fallbackFor(ch, font) ?? font) : font;
            canvas.drawText(ch, px + i * cellW, baseline, fg, f);
          }
        } else {
          canvas.drawText(text, px, baseline, fg, font);
        }
      }
      if (s.underline) {
        const uy = baseline + 2;
        canvas.drawLine(px, uy, px + width, uy, this.paint(s.fg, s.dim ? 0.55 : 1, 1));
        if (s.underline === 2) canvas.drawLine(px, uy + 2, px + width, uy + 2, this.paint(s.fg, 1, 1));
      }
      if (s.strike) {
        const sy = Math.round(cellH / 2);
        canvas.drawLine(px, sy, px + width, sy, this.paint(s.fg, 1, 1));
      }
    }
    return rec.finishRecordingAsPicture();
  }

  /** Composite: background, rows, cursor, at `zoom`, panned by `offsetX`/`offsetY` (screen px). */
  compose(
    grid: Grid,
    view: { width: number; height: number; zoom: number; offsetX: number; offsetY: number; showCursor: boolean },
  ): SkPicture {
    const { cellW, cellH } = this.metrics;
    const theme = this.theme;
    const rec = Skia.PictureRecorder();
    const canvas = rec.beginRecording(Skia.XYWHRect(0, 0, view.width, view.height));
    canvas.drawColor(Skia.Color(hex(theme?.bg ?? 0)));
    canvas.save();
    canvas.translate(-view.offsetX, -view.offsetY);
    canvas.scale(view.zoom, view.zoom);
    const rowH = cellH * view.zoom;
    const first = Math.max(0, Math.floor(view.offsetY / rowH));
    const last = Math.min(this.rowPics.length, Math.ceil((view.offsetY + view.height) / rowH) + 1);
    for (let y = first; y < last; y++) {
      const pic = this.rowPics[y];
      if (!pic) continue;
      canvas.save();
      canvas.translate(0, y * cellH);
      canvas.drawPicture(pic);
      canvas.restore();
    }
    const c = grid.cursor;
    if (view.showCursor && c && c.visible && c.y < grid.rows && c.x < grid.cols) {
      const x = c.x * cellW;
      const y = c.y * cellH;
      const color = theme?.fg ?? 0xffffff;
      const shape = c.shape;
      if (shape === 3 || shape === 4) canvas.drawRect(Skia.XYWHRect(x, y + cellH - 2, cellW, 2), this.paint(color));
      else if (shape === 5 || shape === 6) canvas.drawRect(Skia.XYWHRect(x, y, 2, cellH), this.paint(color));
      else canvas.drawRect(Skia.XYWHRect(x, y, cellW, cellH), this.paint(color, 0.45));
    }
    canvas.restore();
    return rec.finishRecordingAsPicture();
  }
}
