// Rolling-window counters for the perf overlay. Pure; the clock is injected.

export type Clock = () => number;

export const defaultClock: Clock = () =>
  typeof performance !== 'undefined' && typeof performance.now === 'function' ? performance.now() : Date.now();

/** Sum and count of samples over the last `windowMs`. */
export class Window {
  private times: number[] = [];
  private values: number[] = [];
  private head = 0;

  constructor(
    private readonly windowMs = 1000,
    private readonly clock: Clock = defaultClock,
  ) {}

  add(value = 1, at = this.clock()): void {
    this.times.push(at);
    this.values.push(value);
    this.prune(at);
  }

  private prune(now: number): void {
    const cutoff = now - this.windowMs;
    while (this.head < this.times.length && this.times[this.head] < cutoff) this.head++;
    if (this.head > 512 && this.head * 2 > this.times.length) {
      this.times = this.times.slice(this.head);
      this.values = this.values.slice(this.head);
      this.head = 0;
    }
  }

  /** Samples in the window. */
  count(now = this.clock()): number {
    this.prune(now);
    return this.times.length - this.head;
  }

  sum(now = this.clock()): number {
    this.prune(now);
    let s = 0;
    for (let i = this.head; i < this.values.length; i++) s += this.values[i];
    return s;
  }

  /** Per-second rate of the summed values. */
  ratePerSec(now = this.clock()): number {
    return (this.sum(now) * 1000) / this.windowMs;
  }

  avg(now = this.clock()): number {
    const n = this.count(now);
    return n === 0 ? 0 : this.sum(now) / n;
  }

  max(now = this.clock()): number {
    this.prune(now);
    let m = 0;
    for (let i = this.head; i < this.values.length; i++) if (this.values[i] > m) m = this.values[i];
    return m;
  }
}

/** Last value plus a short rolling average. */
export class Latency {
  last: number | null = null;
  private window: Window;

  constructor(windowMs = 10000, clock: Clock = defaultClock) {
    this.window = new Window(windowMs, clock);
  }

  add(ms: number): void {
    this.last = ms;
    this.window.add(ms);
  }

  avg(): number | null {
    return this.window.count() === 0 ? null : this.window.avg();
  }
}

export interface SurfaceStatsSnapshot {
  /** Frames + deltas applied per second (1 s window). */
  updatesPerSec: number;
  /** Full frames per second. */
  framesPerSec: number;
  /** Inbound surface bytes per second (UTF-8 bytes of the JSON text). */
  bytesPerSec: number;
  /** Average JSON.parse ms per surface message (2 s window). */
  parseMs: number;
  /** Average grid apply ms per message (2 s window). */
  applyMs: number;
  /** Average renderer record ms per message (2 s window); set by the view. */
  recordMs: number;
  /** Max of parse + apply + record in the 2 s window. */
  worstMs: number;
  /** Rows changed per second. */
  rowsPerSec: number;
  /** Frames the server sent beyond our applied seq (in flight). */
  resyncs: number;
}

export class SurfaceStats {
  readonly updates: Window;
  readonly frames: Window;
  readonly bytes: Window;
  readonly parse: Window;
  readonly apply: Window;
  readonly record: Window;
  readonly total: Window;
  readonly rows: Window;
  resyncs = 0;

  constructor(clock: Clock = defaultClock) {
    this.updates = new Window(1000, clock);
    this.frames = new Window(1000, clock);
    this.bytes = new Window(1000, clock);
    this.parse = new Window(2000, clock);
    this.apply = new Window(2000, clock);
    this.record = new Window(2000, clock);
    this.total = new Window(2000, clock);
    this.rows = new Window(1000, clock);
  }

  snapshot(): SurfaceStatsSnapshot {
    return {
      updatesPerSec: this.updates.ratePerSec(),
      framesPerSec: this.frames.ratePerSec(),
      bytesPerSec: this.bytes.ratePerSec(),
      parseMs: this.parse.avg(),
      applyMs: this.apply.avg(),
      recordMs: this.record.avg(),
      worstMs: this.total.max(),
      rowsPerSec: this.rows.ratePerSec(),
      resyncs: this.resyncs,
    };
  }
}
