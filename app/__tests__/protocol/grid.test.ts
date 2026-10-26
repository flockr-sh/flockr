import {
  applyDelta,
  applyFrame,
  emptyGrid,
  Flag,
  gridText,
  lineText,
  resolveColor,
  resolveStyle,
  resolveTheme,
  shiftRows,
  Style,
  SurfaceDelta,
  SurfaceFrame,
  Line,
  xtermColor,
} from '../../src/protocol';
import { fixture } from './fixtures';

describe('grid reducer', () => {
  it('applies the spec frame', () => {
    const g = emptyGrid();
    const ch = applyFrame(g, fixture<SurfaceFrame>('surface-frame'));
    expect(ch.full).toBe(true);
    expect(g.seq).toBe(1);
    expect(g.lines).toHaveLength(40);
    expect(g.styles).toHaveLength(4);
    expect(g.links).toEqual(['https://github.com/flockr-sh/flockr']);
    expect(lineText(g.lines[0], g.cols)).toBe('$ ls');
    expect(lineText(g.lines[2], g.cols)).toBe('ok');
  });

  it('applies the spec delta in order: styles, shift, lines, cursor', () => {
    const g = emptyGrid();
    applyFrame(g, fixture<SurfaceFrame>('surface-frame'));
    const res = applyDelta(g, fixture<SurfaceDelta>('surface-delta'));
    expect(res.ok).toBe(true);
    if (!res.ok) return;
    expect(res.change.shift).toBe(3);
    expect(res.change.rows).toEqual([37, 38, 39]);
    expect(g.seq).toBe(2);
    expect(g.styles).toHaveLength(5);
    expect(g.styles[4]).toEqual([1, -1, 0]);
    const text = gridText(g);
    // Old row 3+ moved up by 3; old rows 0..2 are gone.
    expect(text[0]).toBe('');
    expect(text[37]).toBe('test result: ok');
    expect(text[39]).toBe('$');
    expect(g.scroll?.max).toBe(1837);
  });

  it('rejects a delta with the wrong base and before any frame', () => {
    const g = emptyGrid();
    const d = fixture<SurfaceDelta>('surface-delta');
    expect(applyDelta(g, d)).toEqual({ ok: false, reason: 'no_grid' });
    applyFrame(g, fixture<SurfaceFrame>('surface-frame'));
    expect(applyDelta(g, { ...d, base: 7 })).toEqual({ ok: false, reason: 'base_mismatch' });
    expect(g.seq).toBe(1);
  });

  it('keeps absent cursor/scroll/alt unchanged', () => {
    const g = emptyGrid();
    applyFrame(g, fixture<SurfaceFrame>('surface-frame'));
    const res = applyDelta(g, { t: 'surface.delta', sub: 1, seq: 2, base: 1 });
    expect(res.ok && res.change.cursorChanged).toBe(false);
    expect(g.cursor).toEqual({ x: 2, y: 39, visible: true, shape: 0 });
  });

  it('shifts both ways', () => {
    const a = [1, 2, 3, 4, 5];
    shiftRows(a, 2, () => 0);
    expect(a).toEqual([3, 4, 5, 0, 0]);
    const b = [1, 2, 3, 4, 5];
    shiftRows(b, -2, () => 0);
    expect(b).toEqual([0, 0, 1, 2, 3]);
    const c = [1, 2];
    shiftRows(c, 5, () => 9);
    expect(c).toEqual([9, 9]);
  });

  it('lays out the spec row example with a wide char', () => {
    const ex = fixture<{ styles: Style[]; line: Line }>('surface-row-example');
    expect(lineText(ex.line, 20)).toBe('$ ls main.rs 漢');
  });
});

describe('colors and styles', () => {
  const theme = resolveTheme({ fg: '#d8dee9', bg: '#1e1e2e', palette: ['#000000', '#cc0000', null] });

  it('resolves wire colors', () => {
    expect(resolveColor(-1, theme, true)).toBe(0xd8dee9);
    expect(resolveColor(-1, theme, false)).toBe(0x1e1e2e);
    expect(resolveColor(1, theme, true)).toBe(0xcc0000);
    expect(resolveColor(2, theme, true)).toBe(0x00cd00); // null keeps the xterm default
    expect(resolveColor(16777216 + 0x123456, theme, true)).toBe(0x123456);
    expect(resolveColor(16777216, theme, true)).toBe(0);
    expect(resolveColor(196, theme, true)).toBe(0xff0000);
    expect(resolveColor(232, theme, true)).toBe(0x080808);
    expect(xtermColor(255)).toBe(0xeeeeee);
  });

  it('resolves flags', () => {
    const r = resolveStyle([1, -1, Flag.reverse | Flag.underline | (3 << 9)], theme);
    expect(r.fg).toBe(0x1e1e2e);
    expect(r.bg).toBe(0xcc0000);
    expect(r.paintBg).toBe(true);
    expect(r.underline).toBe(3);
    expect(resolveStyle([-1, -1, 0, 2], theme).link).toBe(2);
    expect(resolveStyle([-1, -1, Flag.hidden | Flag.strikethrough], theme)).toMatchObject({
      hidden: true,
      strike: true,
    });
  });
});
