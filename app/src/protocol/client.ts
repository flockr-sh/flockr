// mobile.v2 gateway client. No React deps: runs in React Native and Node.
// Every message rides in a Noise_NK channel pinned to the server key from
// pairing (see noise.ts).
import { applyDelta, applyFrame, emptyGrid, Grid, GridChange, resolveTheme, ResolvedTheme } from './grid';
import { FrameError, HANDSHAKE_MSG_LEN, MessageReader, NoiseInitiator, sealMessage, CipherState } from './noise';
import { HostPort, wsUrl } from './pairing';
import { Clock, defaultClock, Latency, SurfaceStats, Window } from './stats';
import {
  AgentsMessage,
  ApiEventLine,
  ApiResponseLine,
  AppInfo,
  CloseCode,
  ErrorMessage,
  HelloMessage,
  MAX_SERVER_MESSAGE_BYTES,
  NotifyMessage,
  PROTOCOL_VERSION,
  ServerMessage,
  SUBPROTOCOL,
  SurfaceDelta,
  SurfaceEndReason,
  SurfaceFrame,
  SurfaceMode,
  SurfaceStarted,
  TERMINAL_CLOSE_CODES,
  TOKEN_DEAD_CLOSE_CODES,
  WelcomeDevice,
  WelcomeMessage,
  normalizeEndReason,
} from './types';

// --- Socket abstraction (RN WebSocket and the `ws` package both fit) ---

export interface WebSocketLike {
  readonly readyState: number;
  /** The client sets 'arraybuffer'. */
  binaryType?: string;
  send(data: Uint8Array): void;
  close(code?: number, reason?: string): void;
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: { code: number; reason: string }) => void) | null;
  onerror: ((ev: unknown) => void) | null;
}

export type WebSocketFactory = (url: string, protocols: string[]) => WebSocketLike;

const WS_OPEN = 1;

export type Credentials =
  { kind: 'auth'; device_id: string; token: string } | { kind: 'pair'; code: string; device_name: string };

export interface ClientOptions {
  hosts: HostPort[];
  /** Server fingerprint from pairing; `welcome.server.server_id` must match. */
  sid: string;
  /** Server Noise static public key from pairing (32 bytes). The handshake pins it. */
  serverKey: Uint8Array;
  credentials: Credentials;
  app: AppInfo;
  createSocket: WebSocketFactory;
  /** Index into `hosts` to try first (last one that worked). */
  preferredHost?: number;
  maxInFlight?: number;
  /** Reconnect with backoff on non-terminal closes. Default true. */
  reconnect?: boolean;
  /** Per-host connect timeout before trying the next host. */
  connectTimeoutMs?: number;
  /** App-level ping interval for the RTT readout; 0 disables. */
  pingIntervalMs?: number;
  clock?: Clock;
  random?: () => number;
  log?: (msg: string) => void;
  /** Sees each decrypted server message before dispatch (smoke-test byte accounting). */
  onMessageText?: (text: string) => void;
}

export type ConnState = 'idle' | 'connecting' | 'open' | 'waiting' | 'closed' | 'failed';

export interface ConnStatus {
  state: ConnState;
  host?: HostPort;
  hostIndex?: number;
  /** Close code of the last disconnect. */
  code?: number;
  /** Error code name or text for display. */
  error?: string;
  /** Next reconnect attempt delay (state 'waiting'). */
  retryInMs?: number;
}

export interface FatalInfo {
  code: number;
  error: string;
  message?: string;
  /** The stored device token no longer works; the app must re-pair. */
  tokenDead: boolean;
}

/** Error code for a failed Noise handshake (server closed 4006, or message 2 did not decrypt). */
export const HANDSHAKE_FAILED = 'handshake_failed';

export class GatewayError extends Error {
  constructor(
    readonly code: string,
    message?: string,
  ) {
    super(message ? `${code}: ${message}` : code);
  }
}

type Listener<T> = (value: T) => void;

interface ClientEvents {
  status: ConnStatus;
  welcome: WelcomeMessage;
  paired: WelcomeDevice & { token: string };
  fatal: FatalInfo;
  notify: NotifyMessage;
  error: ErrorMessage;
  bye: string;
  rtt: number;
}

interface Pending {
  resolve: (msg: ServerMessage) => void;
  reject: (err: Error) => void;
  /** Reply types that resolve this request. */
  expect: string;
  sentAt: number;
}

interface StreamHandle {
  onResponse?: (line: ApiResponseLine) => void;
  onEvent: (line: ApiEventLine) => void;
  onEnd?: (error?: Error) => void;
}

export interface SubscribeOptions {
  mode?: SurfaceMode;
  cols?: number;
  rows?: number;
  maxFps?: number;
  /** Ack as soon as the update is applied. The renderer turns this off and acks after drawing. */
  autoAck?: boolean;
}

export interface SurfaceHandlers {
  onStarted?: (started: SurfaceStarted) => void;
  onUpdate?: (change: GridChange, grid: Grid) => void;
  onEnded?: (reason: SurfaceEndReason, message?: string) => void;
}

export type InputKind = 'text' | 'key' | 'paste' | 'submit';

export class Subscription {
  readonly stats: SurfaceStats;
  readonly echo: Latency;
  grid: Grid = emptyGrid();
  theme: ResolvedTheme = resolveTheme();
  started: SurfaceStarted | null = null;
  ended: SurfaceEndReason | null = null;
  handlers: SurfaceHandlers = {};
  private ackedSeq = 0;
  private inputPendingAt: number | null = null;

  constructor(
    private readonly client: GatewayClient,
    readonly sub: number,
    readonly target: string,
    public opts: SubscribeOptions,
    private readonly clock: Clock,
  ) {
    this.stats = new SurfaceStats(clock);
    this.echo = new Latency(10000, clock);
  }

  get mode(): SurfaceMode {
    return this.opts.mode ?? 'observe';
  }

  /** Message that (re)opens this subscription. */
  subscribeMessage(id: string): Record<string, unknown> {
    const msg: Record<string, unknown> = {
      t: 'surface.subscribe',
      id,
      sub: this.sub,
      target: this.target,
      mode: this.mode,
      max_fps: this.opts.maxFps ?? 30,
    };
    if (this.mode === 'fit') {
      msg.cols = this.opts.cols;
      msg.rows = this.opts.rows;
    }
    return msg;
  }

  /** @internal */
  resetForResubscribe(): void {
    this.grid = emptyGrid();
    this.ackedSeq = 0;
  }

  /** @internal */
  handleStarted(msg: SurfaceStarted): void {
    this.started = msg;
    this.theme = resolveTheme(msg.theme);
    this.handlers.onStarted?.(msg);
  }

  /** @internal */
  handleUpdate(msg: SurfaceFrame | SurfaceDelta, bytes: number, parseMs: number): void {
    if (this.ended) return;
    const t0 = this.clock();
    let change: GridChange;
    if (msg.t === 'surface.frame') {
      change = applyFrame(this.grid, msg);
      this.stats.frames.add(1, t0);
    } else {
      const res = applyDelta(this.grid, msg);
      if (!res.ok) {
        this.stats.resyncs++;
        this.client.sendRaw({ t: 'surface.resync', sub: this.sub });
        return;
      }
      change = res.change;
    }
    const t1 = this.clock();
    const applyMs = t1 - t0;
    this.stats.updates.add(1, t1);
    this.stats.bytes.add(bytes, t1);
    this.stats.parse.add(parseMs, t1);
    this.stats.apply.add(applyMs, t1);
    this.stats.rows.add(change.full ? this.grid.rows : change.rows.length, t1);
    this.lastParseApplyMs = parseMs + applyMs;
    if (this.inputPendingAt !== null && (change.full || change.rows.length > 0 || change.cursorChanged)) {
      this.echo.add(t1 - this.inputPendingAt);
      this.inputPendingAt = null;
    }
    this.handlers.onUpdate?.(change, this.grid);
    if (this.opts.autoAck !== false) this.ack();
  }

  private lastParseApplyMs = 0;

  /** Renderer reports how long recording the last update took. */
  reportRecord(ms: number): void {
    const now = this.clock();
    this.stats.record.add(ms, now);
    this.stats.total.add(this.lastParseApplyMs + ms, now);
  }

  /** Ack the newest applied seq (cumulative). */
  ack(): void {
    const seq = this.grid.seq;
    if (seq > this.ackedSeq) {
      this.ackedSeq = seq;
      this.client.sendRaw({ t: 'surface.ack', sub: this.sub, seq });
    }
  }

  /** @internal */
  handleEnded(reason: SurfaceEndReason, message?: string): void {
    if (this.ended) return;
    this.ended = reason;
    this.handlers.onEnded?.(reason, message);
  }

  resync(): void {
    this.client.sendRaw({ t: 'surface.resync', sub: this.sub });
  }

  resize(cols: number, rows: number): void {
    this.opts = { ...this.opts, cols, rows };
    if (this.mode === 'fit') this.client.sendRaw({ t: 'surface.resize', sub: this.sub, cols, rows });
  }

  scroll(lines: number): void {
    if (lines !== 0) this.client.sendRaw({ t: 'surface.scroll', sub: this.sub, lines });
  }

  scrollToBottom(): void {
    this.client.sendRaw({ t: 'surface.scroll', sub: this.sub, to: 'bottom' });
  }

  async history(lines = 500): Promise<string[]> {
    const reply = await this.client.call({ t: 'surface.history', sub: this.sub, lines }, 'surface.history.result');
    return Array.isArray(reply.lines) ? (reply.lines as string[]) : [];
  }

  /** Send input to this pane. Resolves with the input.ok round trip in ms. */
  input(kind: InputKind, value: string | string[]): Promise<number> {
    if (this.inputPendingAt === null) this.inputPendingAt = this.clock();
    return this.client.input(kind, { sub: this.sub }, value);
  }

  unsubscribe(): void {
    this.client.unsubscribe(this);
  }
}

export class GatewayClient {
  private socket: WebSocketLike | null = null;
  private welcomeMsg: WelcomeMessage | null = null;
  private status: ConnStatus = { state: 'idle' };
  private listeners: { [K in keyof ClientEvents]?: Set<Listener<ClientEvents[K]>> } = {};
  private pending = new Map<string, Pending>();
  private streams = new Map<string, StreamHandle>();
  private subs = new Map<number, Subscription>();
  private nextId = 1;
  private nextSub = 1;
  private attempt = 0;
  private hostIndex: number;
  private hostsTried = 0;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private connectTimer: ReturnType<typeof setTimeout> | null = null;
  private pingTimer: ReturnType<typeof setInterval> | null = null;
  private stopped = false;
  private lastError: ErrorMessage | null = null;
  private agentsWatch: { id: string; cb: (msg: AgentsMessage) => void } | null = null;
  private foreground: boolean | null = null;
  private pings = new Map<number, number>();
  private readonly clock: Clock;
  private readonly random: () => number;
  private credentials: Credentials;
  /** Transport ciphers once the handshake is done; null before. */
  private tx: CipherState | null = null;
  private reader: MessageReader | null = null;
  readonly rtt: Latency;
  readonly inputRtt: Latency;
  readonly bytesIn: Window;
  readonly msgsIn: Window;

  constructor(private readonly opts: ClientOptions) {
    this.clock = opts.clock ?? defaultClock;
    this.random = opts.random ?? Math.random;
    this.credentials = opts.credentials;
    this.hostIndex = Math.min(Math.max(0, opts.preferredHost ?? 0), Math.max(0, opts.hosts.length - 1));
    this.rtt = new Latency(10000, this.clock);
    this.inputRtt = new Latency(10000, this.clock);
    this.bytesIn = new Window(1000, this.clock);
    this.msgsIn = new Window(1000, this.clock);
  }

  // --- events ---

  on<K extends keyof ClientEvents>(event: K, cb: Listener<ClientEvents[K]>): () => void {
    let set = this.listeners[event] as Set<Listener<ClientEvents[K]>> | undefined;
    if (!set) {
      set = new Set();
      (this.listeners as Record<string, unknown>)[event] = set;
    }
    set.add(cb);
    return () => set?.delete(cb);
  }

  private emit<K extends keyof ClientEvents>(event: K, value: ClientEvents[K]): void {
    const set = this.listeners[event] as Set<Listener<ClientEvents[K]>> | undefined;
    if (!set) return;
    for (const cb of Array.from(set)) {
      try {
        cb(value);
      } catch (e) {
        this.log(`listener for ${event} threw: ${String(e)}`);
      }
    }
  }

  private log(msg: string): void {
    this.opts.log?.(msg);
  }

  private setStatus(status: ConnStatus): void {
    this.status = status;
    this.emit('status', status);
  }

  get connStatus(): ConnStatus {
    return this.status;
  }

  get welcome(): WelcomeMessage | null {
    return this.welcomeMsg;
  }

  get ready(): boolean {
    return this.welcomeMsg !== null && this.socket?.readyState === WS_OPEN;
  }

  /** Index of the host that last completed a handshake. */
  get currentHostIndex(): number {
    return this.hostIndex;
  }

  hasFeature(name: string): boolean {
    return !!this.welcomeMsg?.features.includes(name);
  }

  // --- lifecycle ---

  connect(): void {
    this.stopped = false;
    this.attempt = 0;
    this.hostsTried = 0;
    this.openSocket();
  }

  /** Wait for the next `welcome` (or reject on fatal close). */
  waitReady(timeoutMs = 15000): Promise<WelcomeMessage> {
    if (this.ready && this.welcomeMsg) return Promise.resolve(this.welcomeMsg);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        off1();
        off2();
        reject(new GatewayError('timeout', 'no welcome from server'));
      }, timeoutMs);
      const off1 = this.on('welcome', (w) => {
        clearTimeout(timer);
        off1();
        off2();
        resolve(w);
      });
      const off2 = this.on('fatal', (f) => {
        clearTimeout(timer);
        off1();
        off2();
        reject(new GatewayError(f.error, f.message));
      });
    });
  }

  close(): void {
    this.stopped = true;
    this.clearTimers();
    const s = this.socket;
    this.socket = null;
    this.welcomeMsg = null;
    this.tx = this.reader = null;
    if (s) {
      s.onopen = s.onmessage = s.onerror = null;
      s.onclose = null;
      try {
        s.close(CloseCode.normal, 'client closed');
      } catch {
        // already closed
      }
    }
    this.failAll(new GatewayError('closed'));
    this.setStatus({ state: 'closed' });
  }

  private clearTimers(): void {
    if (this.retryTimer) clearTimeout(this.retryTimer);
    if (this.connectTimer) clearTimeout(this.connectTimer);
    if (this.pingTimer) clearInterval(this.pingTimer);
    this.retryTimer = this.connectTimer = this.pingTimer = null;
  }

  private openSocket(): void {
    if (this.stopped) return;
    const hosts = this.opts.hosts;
    if (hosts.length === 0) {
      this.setStatus({ state: 'failed', error: 'no hosts' });
      return;
    }
    const index = this.hostIndex % hosts.length;
    const host = hosts[index];
    const url = wsUrl(host);
    this.lastError = null;
    this.setStatus({ state: 'connecting', host, hostIndex: index });
    let socket: WebSocketLike;
    try {
      socket = this.opts.createSocket(url, [SUBPROTOCOL]);
    } catch (e) {
      this.log(`socket create failed for ${url}: ${String(e)}`);
      this.onSocketGone(null, 0, String(e), false);
      return;
    }
    socket.binaryType = 'arraybuffer';
    this.socket = socket;
    this.tx = this.reader = null;
    let opened = false;
    let noise: NoiseInitiator | null = null;
    // Covers the WebSocket open and the Noise handshake.
    this.connectTimer = setTimeout(() => {
      if (!this.tx && this.socket === socket) {
        this.log(`connect timeout ${url}`);
        socket.onopen = socket.onmessage = socket.onerror = socket.onclose = null;
        try {
          socket.close();
        } catch {
          // ignore
        }
        this.onSocketGone(socket, 0, 'connect timeout', false);
      }
    }, this.opts.connectTimeoutMs ?? 5000);
    socket.onopen = () => {
      opened = true;
      try {
        noise = new NoiseInitiator(this.opts.serverKey);
        socket.send(noise.writeMessage1());
      } catch (e) {
        this.log(`noise setup failed: ${String(e)}`);
        this.abort(socket, CloseCode.handshakeFailed, String(e));
      }
    };
    socket.onmessage = (ev) => {
      if (this.socket !== socket) return;
      const bytes = binaryData(ev.data);
      if (!bytes) {
        this.log('text frame from server');
        this.abort(socket, CloseCode.protocolError, 'text frame');
        return;
      }
      if (this.tx) {
        this.onFrame(socket, bytes);
        return;
      }
      // Noise message 2.
      if (!noise) return this.abort(socket, CloseCode.protocolError, 'frame before handshake');
      try {
        if (bytes.length !== HANDSHAKE_MSG_LEN) throw new Error(`message 2 is ${bytes.length} bytes`);
        const { ciphers } = noise.readMessage2(bytes);
        noise = null;
        this.tx = ciphers.send;
        this.reader = new MessageReader(ciphers.recv, MAX_SERVER_MESSAGE_BYTES);
      } catch (e) {
        this.log(`handshake failed: ${String(e)}`);
        this.abort(socket, CloseCode.handshakeFailed, 'handshake failed');
        return;
      }
      if (this.connectTimer) clearTimeout(this.connectTimer);
      this.connectTimer = null;
      this.sendHello();
    };
    socket.onerror = () => {
      // onclose follows; nothing to do here.
    };
    socket.onclose = (ev) => {
      this.onSocketGone(socket, ev.code, ev.reason, opened);
    };
  }

  private sendHello(): void {
    const hello: HelloMessage = {
      t: 'hello',
      versions: [PROTOCOL_VERSION],
      app: this.opts.app,
      max_in_flight: this.opts.maxInFlight ?? 2,
    };
    const c = this.credentials;
    if (c.kind === 'auth') hello.auth = { device_id: c.device_id, token: c.token };
    else hello.pair = { code: c.code, device_name: c.device_name, sid: this.opts.sid };
    this.sendRaw(hello as unknown as Record<string, unknown>, true);
  }

  /** Close the socket ourselves and handle it as closed with `code`. */
  private abort(socket: WebSocketLike, code: number, reason: string): void {
    if (this.socket !== socket) return;
    socket.onopen = socket.onmessage = socket.onerror = socket.onclose = null;
    try {
      // 4006 is the server's code; the client closes a failed handshake as a protocol error.
      socket.close(code === CloseCode.handshakeFailed ? CloseCode.protocolError : code, reason);
    } catch {
      // already closed
    }
    this.onSocketGone(socket, code, reason, true);
  }

  private onSocketGone(socket: WebSocketLike | null, code: number, reason: string, opened: boolean): void {
    if (socket && this.socket !== socket) return;
    this.socket = null;
    this.tx = this.reader = null;
    const hadWelcome = this.welcomeMsg !== null;
    this.welcomeMsg = null;
    if (this.connectTimer) clearTimeout(this.connectTimer);
    if (this.pingTimer) clearInterval(this.pingTimer);
    this.connectTimer = this.pingTimer = null;
    this.failAll(new GatewayError('disconnected', reason || `close ${code}`));
    if (this.stopped) return;

    const errName = this.lastError?.code ?? closeName(code);
    if (TERMINAL_CLOSE_CODES.has(code)) {
      this.stopped = true;
      const fatal: FatalInfo = {
        code,
        error: errName,
        message: this.lastError?.message,
        tokenDead: TOKEN_DEAD_CLOSE_CODES.has(code) && this.credentials.kind === 'auth',
      };
      this.setStatus({ state: 'failed', code, error: errName });
      this.emit('fatal', fatal);
      return;
    }
    // Not reachable on this host: try the next one at once, then back off
    // after a full pass. This runs before the reconnect:false check so a
    // one-shot connect (pairing) still tries every advertised host.
    if (!opened && !hadWelcome && this.hostsTried + 1 < this.opts.hosts.length) {
      this.hostsTried++;
      this.hostIndex = (this.hostIndex + 1) % this.opts.hosts.length;
      this.openSocket();
      return;
    }
    if (this.opts.reconnect === false) {
      this.stopped = true;
      this.setStatus({ state: 'closed', code, error: errName });
      return;
    }
    if (hadWelcome) this.attempt = 0;
    this.hostsTried = 0;
    const base = Math.min(30000, 500 * 2 ** this.attempt);
    const delay = Math.round(base * (0.8 + 0.4 * this.random()));
    this.attempt++;
    this.setStatus({ state: 'waiting', code, error: errName, retryInMs: delay });
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      this.openSocket();
    }, delay);
  }

  /** Reconnect now (e.g. app came to foreground). */
  reconnectNow(): void {
    if (this.stopped || this.ready) return;
    if (this.retryTimer) {
      clearTimeout(this.retryTimer);
      this.retryTimer = null;
      this.attempt = 0;
      this.openSocket();
    }
  }

  private failAll(err: Error): void {
    const pending = Array.from(this.pending.values());
    this.pending.clear();
    for (const p of pending) p.reject(err);
    const streams = Array.from(this.streams.values());
    this.streams.clear();
    for (const s of streams) s.onEnd?.(err);
  }

  // --- sending ---

  /** @internal Send one message. Returns false if not connected. */
  sendRaw(msg: Record<string, unknown>, beforeWelcome = false): boolean {
    const s = this.socket;
    const tx = this.tx;
    if (!s || !tx || s.readyState !== WS_OPEN) return false;
    if (!beforeWelcome && !this.welcomeMsg) return false;
    for (const frame of sealMessage(tx, JSON.stringify(msg))) s.send(frame);
    return true;
  }

  private newId(prefix: string): string {
    return `${prefix}${this.nextId++}`;
  }

  /** @internal Send a request and wait for a reply of type `expect` (or `error`) with the same id. */
  call(msg: Record<string, unknown>, expect: string, timeoutMs = 30000): Promise<ServerMessage> {
    const id = (msg.id as string | undefined) ?? this.newId('c');
    return new Promise((resolve, reject) => {
      if (!this.sendRaw({ ...msg, id })) {
        reject(new GatewayError('not_connected'));
        return;
      }
      const timer = setTimeout(() => {
        if (this.pending.delete(id)) reject(new GatewayError('timeout', `${String(msg.t)} got no reply`));
      }, timeoutMs);
      this.pending.set(id, {
        expect,
        sentAt: this.clock(),
        resolve: (m) => {
          clearTimeout(timer);
          resolve(m);
        },
        reject: (e) => {
          clearTimeout(timer);
          reject(e);
        },
      });
    });
  }

  // --- API passthrough (9.2) ---

  async request(method: string, params: Record<string, unknown> = {}, timeoutMs = 30000): Promise<ApiResponseLine> {
    const reply = await this.call({ t: 'api.request', method, params }, 'api.response', timeoutMs);
    return (reply.response ?? {}) as ApiResponseLine;
  }

  /** `request` that unwraps `result` and throws on an API error. */
  async api<T = Record<string, unknown>>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    const line = await this.request(method, params);
    if (line.error) throw new GatewayError(line.error.code, line.error.message);
    return (line.result ?? {}) as T;
  }

  stream(method: string, params: Record<string, unknown>, handle: StreamHandle): { id: string; cancel: () => void } {
    const id = this.newId('st');
    this.streams.set(id, handle);
    if (!this.sendRaw({ t: 'api.request', id, method, params })) {
      this.streams.delete(id);
      handle.onEnd?.(new GatewayError('not_connected'));
    }
    return {
      id,
      cancel: () => {
        if (this.streams.has(id)) this.sendRaw({ t: 'api.cancel', id });
      },
    };
  }

  // --- agents.watch (9.3) ---

  watchAgents(cb: (msg: AgentsMessage) => void): () => void {
    const id = this.newId('w');
    this.agentsWatch = { id, cb };
    this.sendRaw({ t: 'agents.watch', id });
    return () => {
      if (this.agentsWatch?.id === id) {
        this.agentsWatch = null;
        this.sendRaw({ t: 'agents.unwatch', id });
      }
    };
  }

  // --- surfaces (9.4, 10) ---

  subscribe(target: string, opts: SubscribeOptions = {}, handlers: SurfaceHandlers = {}): Subscription {
    const sub = this.nextSub++;
    const s = new Subscription(this, sub, target, opts, this.clock);
    s.handlers = handlers;
    this.subs.set(sub, s);
    if (this.ready) this.sendSubscribe(s);
    return s;
  }

  private sendSubscribe(s: Subscription): void {
    s.resetForResubscribe();
    const id = this.newId('s');
    this.call(s.subscribeMessage(id), 'surface.started')
      .then((m) => {
        if (this.subs.get(s.sub) === s) s.handleStarted(m as unknown as SurfaceStarted);
      })
      .catch((e: unknown) => {
        if (e instanceof GatewayError && (e.code === 'disconnected' || e.code === 'closed')) return;
        this.subs.delete(s.sub);
        const err = e as GatewayError;
        s.handleEnded('error', err.message);
      });
  }

  /** @internal */
  unsubscribe(s: Subscription): void {
    if (this.subs.get(s.sub) !== s) return;
    this.subs.delete(s.sub);
    this.sendRaw({ t: 'surface.unsubscribe', sub: s.sub });
    s.handleEnded('unsubscribed');
  }

  /** Change mode or size by re-subscribing under a new sub id. */
  resubscribe(s: Subscription, opts: SubscribeOptions): Subscription {
    const handlers = s.handlers;
    s.handlers = {};
    this.unsubscribe(s);
    return this.subscribe(s.target, { ...s.opts, ...opts }, handlers);
  }

  // --- input (9.6) ---

  async input(kind: InputKind, where: { sub: number } | { target: string }, value: string | string[]): Promise<number> {
    const msg: Record<string, unknown> = { t: `input.${kind}`, ...where };
    if (kind === 'key') msg.keys = Array.isArray(value) ? value : [value];
    else msg.text = Array.isArray(value) ? value.join('') : value;
    const t0 = this.clock();
    await this.call(msg, 'input.ok', 10000);
    const ms = this.clock() - t0;
    this.inputRtt.add(ms);
    return ms;
  }

  // --- push, app state, device (9.7-9.9) ---

  async registerPush(expoToken: string, platform: string): Promise<void> {
    await this.call({ t: 'push.register', expo_token: expoToken, platform }, 'push.ok');
  }

  async unregisterPush(): Promise<void> {
    await this.call({ t: 'push.unregister' }, 'push.ok');
  }

  setForeground(foreground: boolean): void {
    this.foreground = foreground;
    this.sendRaw({ t: 'app.state', foreground });
  }

  async forget(): Promise<void> {
    await this.call({ t: 'device.forget' }, 'device.ok');
    this.close();
  }

  ping(): void {
    const ts = Date.now();
    this.pings.set(ts, this.clock());
    if (this.pings.size > 16) {
      const first = this.pings.keys().next().value;
      if (first !== undefined) this.pings.delete(first);
    }
    this.sendRaw({ t: 'ping', ts });
  }

  // --- receiving ---

  private onFrame(socket: WebSocketLike, frame: Uint8Array): void {
    const reader = this.reader;
    if (!reader) return;
    let text: string | null;
    const t0 = this.clock();
    try {
      text = reader.push(frame);
      this.decryptMs += this.clock() - t0;
    } catch (e) {
      const code = e instanceof FrameError ? e.closeCode : CloseCode.protocolError;
      this.log(`bad frame: ${String(e)}`);
      this.abort(socket, code, (e as Error).message);
      return;
    }
    if (text !== null) {
      const decryptMs = this.decryptMs;
      this.decryptMs = 0;
      this.onMessage(text, reader.lastSize, decryptMs);
    }
  }

  /** Decrypt time of the chunks of the message being reassembled. */
  private decryptMs = 0;

  private onMessage(data: string, bytes: number, decryptMs: number): void {
    this.opts.onMessageText?.(data);
    const t0 = this.clock();
    this.bytesIn.add(bytes, t0);
    this.msgsIn.add(1, t0);
    let msg: ServerMessage;
    try {
      msg = JSON.parse(data) as ServerMessage;
    } catch {
      this.log('dropping non-JSON message');
      return;
    }
    // Decrypt counts as parse: both are per-message receive cost before the grid is touched.
    const parseMs = this.clock() - t0 + decryptMs;
    if (!msg || typeof msg.t !== 'string') return;
    this.dispatch(msg, bytes, parseMs);
  }

  private dispatch(msg: ServerMessage, bytes: number, parseMs: number): void {
    switch (msg.t) {
      case 'welcome':
        this.onWelcome(msg as unknown as WelcomeMessage);
        return;
      case 'error': {
        const err = msg as unknown as ErrorMessage;
        if (err.id) {
          const p = this.pending.get(err.id);
          if (p) {
            this.pending.delete(err.id);
            p.reject(new GatewayError(err.code, err.message));
            return;
          }
          const st = this.streams.get(err.id);
          if (st) {
            this.streams.delete(err.id);
            st.onEnd?.(new GatewayError(err.code, err.message));
            return;
          }
        }
        this.lastError = err;
        this.emit('error', err);
        return;
      }
      case 'surface.frame':
      case 'surface.delta': {
        const s = this.subs.get(msg.sub as number);
        if (s) s.handleUpdate(msg as unknown as SurfaceFrame | SurfaceDelta, bytes, parseMs);
        return;
      }
      case 'surface.ended': {
        const s = this.subs.get(msg.sub as number);
        if (!s) return;
        const reason = normalizeEndReason(msg.reason);
        // The server is stopping or handing off; the gateway closes 4009
        // next. Keep the sub so the next welcome resubscribes it.
        if (reason === 'server_shutdown') return;
        this.subs.delete(s.sub);
        s.handleEnded(reason);
        return;
      }
      case 'api.response': {
        const id = msg.id ?? '';
        const st = this.streams.get(id);
        if (st) {
          st.onResponse?.((msg.response ?? {}) as ApiResponseLine);
          return;
        }
        break;
      }
      case 'api.event': {
        const st = this.streams.get(msg.id ?? '');
        if (st) st.onEvent((msg.event ?? {}) as ApiEventLine);
        return;
      }
      case 'api.end': {
        const id = msg.id ?? '';
        const st = this.streams.get(id);
        if (st) {
          this.streams.delete(id);
          st.onEnd?.();
        }
        return;
      }
      case 'agents': {
        const w = this.agentsWatch;
        if (w && msg.id === w.id) w.cb(msg as unknown as AgentsMessage);
        return;
      }
      case 'pong': {
        const sentAt = this.pings.get(msg.ts as number);
        if (sentAt !== undefined) {
          this.pings.delete(msg.ts as number);
          const ms = this.clock() - sentAt;
          this.rtt.add(ms);
          this.emit('rtt', ms);
        }
        return;
      }
      case 'notify':
        if (msg.kind === 'blocked' || msg.kind === 'done') this.emit('notify', msg as unknown as NotifyMessage);
        return;
      case 'bye':
        this.emit('bye', String(msg.reason ?? ''));
        return;
      default:
        break;
    }
    // Generic id-matched replies.
    if (msg.id) {
      const p = this.pending.get(msg.id);
      if (p && p.expect === msg.t) {
        this.pending.delete(msg.id);
        p.resolve(msg);
      }
    }
  }

  private onWelcome(w: WelcomeMessage): void {
    if (w.server?.server_id !== this.opts.sid) {
      this.lastError = {
        t: 'error',
        code: 'server_mismatch',
        message: 'this address is now a different flockr server',
      };
      this.stopped = true;
      const s = this.socket;
      this.socket = null;
      if (s) {
        s.onclose = null;
        s.close(CloseCode.normal, 'server mismatch');
      }
      this.setStatus({ state: 'failed', error: 'server_mismatch' });
      this.emit('fatal', {
        code: 0,
        error: 'server_mismatch',
        message: 'This address is now a different flockr server.',
        tokenDead: false,
      });
      return;
    }
    this.welcomeMsg = w;
    this.attempt = 0;
    this.hostsTried = 0;
    if (this.credentials.kind === 'pair') {
      if (w.device?.token) {
        this.credentials = { kind: 'auth', device_id: w.device.device_id, token: w.device.token };
        this.emit('paired', { ...w.device, token: w.device.token });
      }
    }
    this.setStatus({ state: 'open', host: this.opts.hosts[this.hostIndex], hostIndex: this.hostIndex });
    this.emit('welcome', w);
    if (this.foreground !== null) this.sendRaw({ t: 'app.state', foreground: this.foreground });
    if (this.agentsWatch) this.sendRaw({ t: 'agents.watch', id: this.agentsWatch.id });
    for (const s of this.subs.values()) this.sendSubscribe(s);
    const interval = this.opts.pingIntervalMs ?? 5000;
    if (interval > 0) {
      this.ping();
      this.pingTimer = setInterval(() => this.ping(), interval);
    }
  }
}

export function closeName(code: number): string {
  switch (code) {
    case 1000:
      return 'normal';
    case 1006:
      return 'connection_lost';
    case 1009:
      return 'too_big';
    case 4000:
      return 'protocol_error';
    case 4001:
      return 'unauthorized';
    case 4002:
      return 'unsupported_version';
    case 4003:
      return 'revoked';
    case 4004:
      return 'too_many_connections';
    case 4005:
      return 'pairing_failed';
    case 4006:
      return HANDSHAKE_FAILED;
    case 4008:
      return 'hello_timeout';
    case 4009:
      return 'server_shutdown';
    case 4029:
      return 'rate_limited';
    default:
      return code ? `close_${code}` : 'unreachable';
  }
}

/** Bytes of a binary WebSocket message (ArrayBuffer in RN/browsers, Buffer/views in Node), or null for text. */
function binaryData(data: unknown): Uint8Array | null {
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  return null;
}
