// The mock's server-side encoder and the app's reducer must agree: random
// terminal edits, encoded as frames/deltas and applied, reproduce the screen.
import { applyDelta, applyFrame, emptyGrid, gridText, SurfaceDelta, SurfaceFrame } from '../../src/protocol';
import { SurfaceEncoder } from '../../mock/encoder';
import { FakeTerminal } from '../../mock/terminal';

function rng(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 2 ** 32;
  };
}

function termText(t: FakeTerminal): string[] {
  return Array.from({ length: t.rows }, (_, y) => t.rowText(y));
}

describe.each([true, false])('encoder round trip (shift=%s)', (allowShift) => {
  it('reproduces random edits', () => {
    const rand = rng(allowShift ? 7 : 11);
    const t = new FakeTerminal(30, 12);
    const enc = new SurfaceEncoder(allowShift);
    const g = emptyGrid();
    let shifts = 0;
    const words = ['ok', 'FAILED', '漢字', '🙂', 'é', 'test', '│', '─'];
    for (let step = 0; step < 400; step++) {
      const r = rand();
      if (r < 0.5)
        t.writeLine(`${step} ${words[Math.floor(rand() * words.length)]}`, [
          Math.floor(rand() * 8) - 1,
          -1,
          rand() < 0.3 ? 1 : 0,
        ]);
      else if (r < 0.8)
        t.put(Math.floor(rand() * 25), Math.floor(rand() * 12), words[Math.floor(rand() * words.length)], [
          16777216 + Math.floor(rand() * 0xffffff),
          -1,
          0,
        ]);
      else if (r < 0.85) t.clear();
      else if (r < 0.87) t.resize(20 + Math.floor(rand() * 20), 8 + Math.floor(rand() * 8));
      const msg = enc.encode(1, t.cells, t.cols, {
        cursor: { x: t.cursorX, y: t.cursorY, visible: true, shape: 0 },
        scroll: null,
        alt: false,
      });
      if (!msg) continue;
      if (msg.t === 'surface.frame') applyFrame(g, msg as SurfaceFrame);
      else {
        const res = applyDelta(g, msg as SurfaceDelta);
        expect(res.ok).toBe(true);
        if ((msg as SurfaceDelta).shift) shifts++;
      }
      expect(g.cols).toBe(t.cols);
      expect(gridText(g)).toEqual(termText(t));
      expect(g.cursor).toEqual({ x: t.cursorX, y: t.cursorY, visible: true, shape: 0 });
    }
    if (allowShift) expect(shifts).toBeGreaterThan(10);
    else expect(shifts).toBe(0);
  });
});
