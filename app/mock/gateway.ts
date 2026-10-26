// Mock mobile.v2 gateway (Node + ws). Used by the jest tests and by
// `npm run mock-gateway` to try the app without a flockr server.
// It follows docs/mobile/protocol-v1.md plus the v2 Noise transport closely
// enough to exercise the app's client; it is not a reference implementation
// of the Rust server.
import { createHash, randomBytes } from 'node:crypto';
import type { AddressInfo } from 'node:net';
import { WebSocket, WebSocketServer } from 'ws';
import {
  CipherState,
  FrameError,
  generateKeyPair,
  HANDSHAKE_MSG_LEN,
  KeyPair,
  keyPairFromPrivate,
  MessageReader,
  NOISE_MAX_MESSAGE,
  NoiseResponder,
  sealMessage,
} from '../src/protocol/noise';
import { base64UrlEncode, encodePairingUrl } from '../src/protocol/pairing';
import type { AgentInfo, AgentStatus, Style, TabInfo, WorkspaceInfo } from '../src/protocol/types';
import { SurfaceEncoder } from './encoder';
import { FakeTerminal } from './terminal';

const SUBPROTOCOL = 'flockr.mobile.v2';
const WS_PATH = '/mobile/v2';
/** v1 inbound caps: 1 MiB for input text messages, 64 KiB for the rest. */
const MAX_INPUT_BYTES = 1024 * 1024;
const MAX_OTHER_BYTES = 64 * 1024;
const INPUT_TYPES = new Set(['input.paste', 'input.submit', 'input.text']);

export type PaneKind = 'shell' | 'log' | 'tui' | 'stress';

export interface MockPane {
  pane_id: string;
  terminal_id: string;
  workspace_id: string;
  tab_id: string;
  agent: string;
  status: AgentStatus;
  kind: PaneKind;
  term: FakeTerminal;
  inputLine: string;
  /** Desktop size for fit-mode restore. */
  desktopSize: [number, number];
  attachOwner: string | null;
  stateChangeSeq: number;
}

export interface MockOptions {
  host?: string;
  port?: number;
  name?: string;
  /** Output rate of the `log` pane in lines per second. */
  logLinesPerSec?: number;
  /** Frame rate of the `stress` pane (whole-screen redraw). */
  stressFps?: number;
  /** Start the content generators. Tests turn this off and drive panes by hand. */
  animate?: boolean;
  /** Emit `shift` in deltas. */
  shift?: boolean;
  /** List `surface.wheel` in features (default true). */
  wheel?: boolean;
  /** Noise static private key (32 bytes). Random per instance by default. */
  staticKey?: Uint8Array;
  log?: (msg: string) => void;
}

interface Device {
  device_id: string;
  name: string;
  platform: string;
  token_sha256: string;
  expo_token?: string;
}

interface Sub {
  sub: number;
  pane: MockPane;
  mode: 'observe' | 'fit';
  encoder: SurfaceEncoder;
  maxFps: number;
  lastSentAt: number;
  ackedSeq: number;
  lastVersion: number;
  forceFull: boolean;
  timer: NodeJS.Timeout | null;
}

interface Conn {
  ws: WebSocket;
  /** Transport ciphers after the Noise handshake. */
  tx: CipherState | null;
  reader: MessageReader | null;
  device: Device | null;
  maxInFlight: number;
  subs: Map<number, Sub>;
  foreground: boolean;
  watchId: string | null;
  lastAgents: string;
  rev: number;
}

const sha256 = (s: string) => createHash('sha256').update(s, 'utf8').digest('hex');
const b64url = (b: Buffer) => b.toString('base64url');

export class MockGateway {
  readonly sid: string;
  readonly name: string;
  readonly bootId = `b-${randomBytes(4).toString('hex')}`;
  /** Noise static key; its public half goes in the pairing payload as `pk`. */
  readonly staticKey: KeyPair;
  readonly panes: MockPane[] = [];
  readonly workspaces: WorkspaceInfo[];
  readonly tabs: TabInfo[];
  readonly devices: Device[] = [];
  /** Every message received from clients, for tests. */
  readonly received: Record<string, unknown>[] = [];
  private pairing: { code_sha256: string; exp: number; failed: number; consumed: boolean } | null = null;
  private wss: WebSocketServer | null = null;
  private conns = new Set<Conn>();
  private timers: NodeJS.Timeout[] = [];
  private tick = 0;
  private stressFrame = 0;
  port = 0;
  host = '127.0.0.1';

  constructor(private readonly opts: MockOptions = {}) {
    this.sid = b64url(randomBytes(16));
    this.staticKey = opts.staticKey ? keyPairFromPrivate(opts.staticKey) : generateKeyPair();
    this.name = opts.name ?? 'mock-flockr';
    this.workspaces = [
      {
        workspace_id: 'w_1',
        number: 1,
        label: 'flockr',
        focused: true,
        agent_status: 'blocked',
        active_tab_id: 'tab_1',
        pane_count: 3,
        tab_count: 2,
      },
      {
        workspace_id: 'w_2',
        number: 2,
        label: 'perf',
        focused: false,
        agent_status: 'working',
        active_tab_id: 'tab_3',
        pane_count: 1,
        tab_count: 1,
      },
    ];
    this.tabs = [
      {
        tab_id: 'tab_1',
        workspace_id: 'w_1',
        number: 1,
        label: 'mobile',
        focused: true,
        pane_count: 2,
        agent_status: 'blocked',
      },
      {
        tab_id: 'tab_2',
        workspace_id: 'w_1',
        number: 2,
        label: 'shell',
        focused: false,
        pane_count: 1,
        agent_status: 'idle',
      },
      {
        tab_id: 'tab_3',
        workspace_id: 'w_2',
        number: 1,
        label: 'stress',
        focused: true,
        pane_count: 1,
        agent_status: 'working',
      },
    ];
    this.addPane('p_1', 't_1', 'w_1', 'tab_1', 'Claude', 'blocked', 'tui', 100, 40);
    this.addPane('p_2', 't_2', 'w_1', 'tab_1', 'Codex', 'working', 'log', 100, 40);
    this.addPane('p_3', 't_3', 'w_1', 'tab_2', 'shell', 'idle', 'shell', 100, 40);
    this.addPane('p_4', 't_4', 'w_2', 'tab_3', 'stress', 'working', 'stress', 120, 50);
  }

  private log(msg: string): void {
    this.opts.log?.(msg);
  }

  private addPane(
    pane_id: string,
    terminal_id: string,
    workspace_id: string,
    tab_id: string,
    agent: string,
    status: AgentStatus,
    kind: PaneKind,
    cols: number,
    rows: number,
  ): MockPane {
    const pane: MockPane = {
      pane_id,
      terminal_id,
      workspace_id,
      tab_id,
      agent,
      status,
      kind,
      term: new FakeTerminal(cols, rows),
      inputLine: '',
      desktopSize: [cols, rows],
      attachOwner: null,
      stateChangeSeq: 1,
    };
    this.panes.push(pane);
    this.seedPane(pane);
    return pane;
  }

  private seedPane(p: MockPane): void {
    const t = p.term;
    if (p.kind === 'shell') {
      t.writeLine('mock flockr shell. Type something and press Enter.', [8, -1, 0]);
      t.write('$ ', [2, -1, 1]);
    } else if (p.kind === 'tui') {
      this.drawTui(p);
    } else if (p.kind === 'log') {
      t.writeLine('cargo test --workspace', [6, -1, 1]);
    }
  }

  private drawTui(p: MockPane): void {
    const t = p.term;
    t.clear();
    const w = t.cols;
    t.put(0, 0, '╭' + '─'.repeat(w - 2) + '╮', [8, -1, 0]);
    t.put(2, 1, 'Claude wants to run: ', [-1, -1, 1]);
    t.put(23, 1, 'cargo nextest run', [16777216 + 0xffb86c, -1, 0]);
    t.put(2, 3, '❯ 1. Yes', [2, -1, 1]);
    t.put(2, 4, '  2. Yes, and don’t ask again', [-1, -1, 0]);
    t.put(2, 5, '  3. No, tell Claude what to do differently (esc)', [-1, -1, 0]);
    t.put(2, 7, 'wide: 漢字 emoji: 🙂 accent: é', [5, -1, 4]);
    t.put(2, 8, 'underline', [-1, -1, 8]);
    t.put(14, 8, 'reverse', [-1, -1, 64]);
    t.put(24, 8, 'dim', [-1, -1, 2]);
    t.put(30, 8, 'strike', [-1, -1, 256]);
    t.put(40, 8, 'truecolor bg', [0, 16777216 + 0x3a5fcd, 0]);
    t.put(0, t.rows - 1, '╰' + '─'.repeat(w - 2) + '╯', [8, -1, 0]);
    t.cursorX = 4;
    t.cursorY = 3;
  }

  private animateStep(): void {
    this.tick++;
    for (const p of this.panes) {
      if (p.kind === 'log') {
        const n = Math.max(1, Math.round((this.opts.logLinesPerSec ?? 20) / 20));
        for (let i = 0; i < n; i++) {
          const k = this.tick * n + i;
          const ok = k % 7 !== 0;
          p.term.write(`test mobile::cells::case_${k} ... `);
          p.term.writeLine(ok ? 'ok' : 'FAILED', ok ? [2, -1, 1] : [1, -1, 1]);
        }
      }
    }
  }

  private stressStep(): void {
    const p = this.panes.find((x) => x.kind === 'stress');
    if (!p) return;
    const t = p.term;
    const f = ++this.stressFrame;
    for (let y = 0; y < t.rows; y++) {
      let line = '';
      for (let x = 0; x < t.cols; x++) line += String.fromCharCode(33 + ((x + y + f) % 90));
      const style: Style = [
        16777216 + (((y * 5 + f * 3) & 0xff) << 16) + (((x0(y) + f) & 0xff) << 8) + 0x80,
        -1,
        y % 3 === 0 ? 1 : 0,
      ];
      t.put(0, y, line, style);
    }
    t.put(2, 0, ` frame ${f} `, [-1, 1, 1]);
  }

  async start(): Promise<{ host: string; port: number }> {
    const host = this.opts.host ?? '127.0.0.1';
    this.wss = new WebSocketServer({
      host,
      port: this.opts.port ?? 0,
      path: WS_PATH,
      maxPayload: NOISE_MAX_MESSAGE,
      handleProtocols: (protocols) => (protocols.has(SUBPROTOCOL) ? SUBPROTOCOL : false),
    });
    await new Promise<void>((resolve, reject) => {
      this.wss?.once('listening', () => resolve());
      this.wss?.once('error', reject);
    });
    const addr = this.wss.address() as AddressInfo;
    this.port = addr.port;
    this.host = host;
    this.wss.on('connection', (ws) => this.onConnection(ws));
    if (this.opts.animate !== false) {
      this.timers.push(setInterval(() => this.animateStep(), 50));
      const stressFps = this.opts.stressFps ?? 30;
      if (stressFps > 0) this.timers.push(setInterval(() => this.stressStep(), 1000 / stressFps));
      this.timers.push(setInterval(() => this.pumpAll(), 5));
    }
    return { host, port: this.port };
  }

  async stop(): Promise<void> {
    for (const t of this.timers) clearInterval(t);
    this.timers = [];
    for (const c of this.conns) {
      for (const s of c.subs.values()) if (s.timer) clearTimeout(s.timer);
      c.ws.terminate();
    }
    this.conns.clear();
    await new Promise<void>((resolve) => (this.wss ? this.wss.close(() => resolve()) : resolve()));
    this.wss = null;
  }

  /** New single-use pairing code; returns the flockr:// URL. */
  createPairing(ttlSecs = 600, hosts?: string[]): string {
    const code = b64url(randomBytes(16));
    const exp = Math.floor(Date.now() / 1000) + ttlSecs;
    this.pairing = { code_sha256: sha256(code), exp, failed: 0, consumed: false };
    return encodePairingUrl({
      v: 2,
      name: this.name,
      hosts: hosts ?? [this.host],
      port: this.port,
      sid: this.sid,
      pk: this.pk,
      code,
      exp,
    });
  }

  /** Server public key as it appears in the pairing payload. */
  get pk(): string {
    return base64UrlEncode(this.staticKey.publicKey);
  }

  /** Send a message to every connected client, then optionally close (tests). */
  sendToAll(msg: Record<string, unknown>, closeCode?: number): void {
    for (const c of this.conns) {
      this.send(c, msg);
      if (closeCode !== undefined) this.endConn(c, closeCode, 'test');
    }
  }

  revoke(device_id: string): void {
    const i = this.devices.findIndex((d) => d.device_id === device_id);
    if (i >= 0) this.devices.splice(i, 1);
    for (const c of this.conns) {
      if (c.device?.device_id === device_id) {
        this.send(c, { t: 'error', code: 'revoked' });
        this.endConn(c, 4003, 'revoked');
      }
    }
  }

  /** Change an agent's status (agents watch + notify). */
  setStatus(pane_id: string, status: AgentStatus): void {
    const p = this.panes.find((x) => x.pane_id === pane_id);
    if (!p || p.status === status) return;
    p.status = status;
    p.stateChangeSeq++;
    this.broadcastAgents();
    if (status === 'blocked' || status === 'done') {
      const ws = this.workspaces.find((w) => w.workspace_id === p.workspace_id);
      const tab = this.tabs.find((t) => t.tab_id === p.tab_id);
      for (const c of this.conns) {
        if (c.device && c.foreground) {
          this.send(c, {
            t: 'notify',
            kind: status,
            pane_id: p.pane_id,
            workspace_id: p.workspace_id,
            tab_id: p.tab_id,
            terminal_id: p.terminal_id,
            title: status === 'blocked' ? `${p.agent} is blocked` : `${p.agent} finished`,
            body: `${ws?.label ?? p.workspace_id} › ${tab?.label ?? p.tab_id} · ${this.name}`,
          });
        }
      }
    }
  }

  pane(id: string): MockPane | undefined {
    return this.panes.find((p) => p.pane_id === id || p.terminal_id === id);
  }

  /** Force pending surface updates out now (tests). */
  pumpAll(): void {
    for (const c of this.conns) for (const s of c.subs.values()) this.pump(c, s);
  }

  // --- connection handling ---

  private onConnection(ws: WebSocket): void {
    const conn: Conn = {
      ws,
      tx: null,
      reader: null,
      device: null,
      maxInFlight: 2,
      subs: new Map(),
      foreground: false,
      watchId: null,
      lastAgents: '',
      rev: 0,
    };
    this.conns.add(conn);
    const helloTimer = setTimeout(() => {
      if (!conn.device) this.endConn(conn, 4008, 'hello_timeout');
    }, 10000);
    const noise = new NoiseResponder(this.staticKey);
    ws.on('message', (data, isBinary) => {
      if (!isBinary) return this.endConn(conn, 4000, 'text frame');
      const buf = data as Buffer;
      const bytes = new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
      if (!conn.tx || !conn.reader) {
        try {
          if (bytes.length !== HANDSHAKE_MSG_LEN) throw new Error(`message 1 is ${bytes.length} bytes`);
          noise.readMessage1(bytes);
          const { message, ciphers } = noise.writeMessage2();
          ws.send(message);
          conn.tx = ciphers.send;
          conn.reader = new MessageReader(ciphers.recv, MAX_INPUT_BYTES);
        } catch (e) {
          this.log(`handshake failed: ${String(e)}`);
          this.endConn(conn, 4006, 'handshake_failed');
        }
        return;
      }
      let text: string | null;
      try {
        text = conn.reader.push(bytes);
      } catch (e) {
        return this.endConn(conn, e instanceof FrameError ? e.closeCode : 4000, (e as Error).message);
      }
      if (text === null) return;
      let msg: Record<string, unknown>;
      try {
        msg = JSON.parse(text);
      } catch {
        return this.endConn(conn, 4000, 'bad json');
      }
      if (conn.reader.lastSize > MAX_OTHER_BYTES && !INPUT_TYPES.has(String(msg.t)))
        return this.endConn(conn, 1009, 'too big');
      if (typeof msg.t !== 'string') return this.endConn(conn, 4000, 'missing t');
      this.received.push(msg);
      if (!conn.device) {
        clearTimeout(helloTimer);
        if (msg.t !== 'hello') return this.endConn(conn, 4000, 'expected hello');
        this.onHello(conn, msg);
        return;
      }
      this.onMessage(conn, msg);
    });
    ws.on('close', () => {
      clearTimeout(helloTimer);
      for (const s of conn.subs.values()) this.endSub(conn, s, null);
      this.conns.delete(conn);
    });
  }

  private send(c: Conn, msg: Record<string, unknown>): void {
    if (c.ws.readyState !== WebSocket.OPEN || !c.tx) return;
    for (const frame of sealMessage(c.tx, JSON.stringify(msg))) c.ws.send(frame);
  }

  private endConn(c: Conn, code: number, reason: string): void {
    try {
      c.ws.close(code, reason);
    } catch {
      c.ws.terminate();
    }
  }

  private fail(c: Conn, code: string, close: number, message?: string): void {
    this.send(c, { t: 'error', code, message });
    this.endConn(c, close, code);
  }

  private onHello(c: Conn, msg: Record<string, unknown>): void {
    const versions = Array.isArray(msg.versions) ? msg.versions : [];
    if (!versions.includes(2)) {
      this.send(c, { t: 'error', code: 'unsupported_version', supported: [2] });
      return this.endConn(c, 4002, 'unsupported_version');
    }
    const auth = msg.auth as { device_id?: string; token?: string } | undefined;
    const pair = msg.pair as { code?: string; device_name?: string; sid?: string } | undefined;
    if (!!auth === !!pair) return this.endConn(c, 4000, 'auth xor pair');
    const mif = typeof msg.max_in_flight === 'number' ? msg.max_in_flight : 2;
    c.maxInFlight = Math.min(8, Math.max(1, mif));
    const app = (msg.app ?? {}) as { platform?: string };
    let token: string | undefined;
    if (pair) {
      const p = this.pairing;
      const now = Math.floor(Date.now() / 1000);
      if (!p || p.consumed || p.exp < now || p.failed >= 5 || pair.sid !== this.sid)
        return this.fail(c, 'pairing_failed', 4005);
      if (sha256(pair.code ?? '') !== p.code_sha256) {
        p.failed++;
        return this.fail(c, 'pairing_failed', 4005);
      }
      p.consumed = true;
      token = `fmd1_${b64url(randomBytes(32))}`;
      const device: Device = {
        device_id: `d_${b64url(randomBytes(12))}`,
        name: (pair.device_name ?? 'phone').trim().slice(0, 64) || 'phone',
        platform: app.platform ?? 'unknown',
        token_sha256: sha256(token),
      };
      this.devices.push(device);
      c.device = device;
      this.log(`paired ${device.name} (${device.device_id})`);
    } else {
      const d = this.devices.find((x) => x.device_id === auth?.device_id);
      if (!d || sha256(auth?.token ?? '') !== d.token_sha256) return this.fail(c, 'unauthorized', 4001);
      c.device = d;
    }
    const device = c.device;
    if (!device) return;
    this.send(c, {
      t: 'welcome',
      version: 2,
      server: { name: this.name, server_id: this.sid, flockr_version: 'mock', boot_id: this.bootId },
      device: { device_id: device.device_id, name: device.name, ...(token ? { token } : {}) },
      features: [
        'api',
        'agents.watch',
        'surface',
        'surface.history',
        'input',
        'push.expo',
        ...(this.opts.shift !== false ? ['surface.shift'] : []),
        ...(this.opts.wheel !== false ? ['surface.wheel'] : []),
      ],
      methods: [
        'ping',
        'agent.list',
        'agent.get',
        'workspace.list',
        'tab.list',
        'pane.read',
        'pane.send_text',
        'pane.send_keys',
        'events.subscribe',
      ],
      limits: {
        max_in_flight: c.maxInFlight,
        max_subscriptions: 4,
        max_message_bytes: 65536,
        max_input_bytes: 1048576,
        max_cols: 500,
        max_rows: 200,
      },
    });
  }

  private agentInfos(): AgentInfo[] {
    return this.panes.map((p) => ({
      terminal_id: p.terminal_id,
      pane_id: p.pane_id,
      workspace_id: p.workspace_id,
      tab_id: p.tab_id,
      agent: p.agent.toLowerCase(),
      display_agent: p.agent,
      agent_status: p.status,
      state_change_seq: p.stateChangeSeq,
      revision: p.stateChangeSeq,
      focused: false,
    }));
  }

  private agentsPayload(): Record<string, unknown> {
    return { agents: this.agentInfos(), workspaces: this.workspaces, tabs: this.tabs };
  }

  private broadcastAgents(): void {
    for (const c of this.conns) this.sendAgents(c, false);
  }

  private sendAgents(c: Conn, force: boolean): void {
    if (!c.watchId) return;
    const payload = this.agentsPayload();
    const key = JSON.stringify(payload);
    if (!force && key === c.lastAgents) return;
    c.lastAgents = key;
    c.rev++;
    this.send(c, { t: 'agents', id: c.watchId, rev: c.rev, ...payload });
  }

  private onMessage(c: Conn, msg: Record<string, unknown>): void {
    const id = typeof msg.id === 'string' ? msg.id : undefined;
    const err = (code: string, message?: string) => this.send(c, { t: 'error', ...(id ? { id } : {}), code, message });
    switch (msg.t) {
      case 'ping':
        return this.send(c, { t: 'pong', ts: msg.ts });
      case 'app.state':
        c.foreground = !!msg.foreground;
        return;
      case 'agents.watch':
        c.watchId = id ?? null;
        return this.sendAgents(c, true);
      case 'agents.unwatch':
        if (c.watchId === id) c.watchId = null;
        return this.send(c, { t: 'api.end', id });
      case 'api.request':
        return this.onApi(c, id ?? '', String(msg.method), (msg.params ?? {}) as Record<string, unknown>);
      case 'api.cancel':
        return this.send(c, { t: 'api.end', id });
      case 'push.register': {
        const tok = String(msg.expo_token ?? '');
        if (!/^(Exponent|Expo)PushToken\[[A-Za-z0-9_-]{1,256}\]$/.test(tok))
          return err('bad_request', 'bad expo token');
        if (c.device) c.device.expo_token = tok;
        this.log(`push token registered …${tok.slice(-6)}`);
        return this.send(c, { t: 'push.ok', id });
      }
      case 'push.unregister':
        if (c.device) delete c.device.expo_token;
        return this.send(c, { t: 'push.ok', id });
      case 'device.forget':
        if (c.device) this.devices.splice(this.devices.indexOf(c.device), 1);
        this.send(c, { t: 'device.ok', id });
        return this.endConn(c, 1000, 'forgotten');
      case 'surface.subscribe':
        return this.onSubscribe(c, msg, id);
      case 'surface.ack': {
        const s = c.subs.get(msg.sub as number);
        if (s) {
          s.ackedSeq = Math.max(s.ackedSeq, msg.seq as number);
          this.pump(c, s);
        }
        return;
      }
      case 'surface.resync': {
        const s = c.subs.get(msg.sub as number);
        if (s) {
          s.forceFull = true;
          s.ackedSeq = s.encoder.seq;
          this.pump(c, s);
        }
        return;
      }
      case 'surface.resize': {
        const s = c.subs.get(msg.sub as number);
        if (!s) return err('not_found');
        if (s.mode !== 'fit') return err('bad_request', 'resize needs fit mode');
        s.pane.term.resize(clamp(msg.cols as number, 2, 500), clamp(msg.rows as number, 2, 200));
        return this.pump(c, s);
      }
      case 'surface.scroll':
      case 'surface.wheel': {
        // The mock has no scrollback viewport and no mouse-reporting apps.
        if (!c.subs.has(msg.sub as number)) return err('not_found');
        return;
      }
      case 'surface.history': {
        const s = c.subs.get(msg.sub as number);
        if (!s) return err('not_found');
        const n = clamp((msg.lines as number) ?? 500, 1, 10000);
        const t = s.pane.term;
        const lines = [...t.scrollback, ...Array.from({ length: t.rows }, (_, y) => t.rowText(y))].slice(-n);
        return this.send(c, { t: 'surface.history.result', id, sub: s.sub, lines });
      }
      case 'surface.unsubscribe': {
        const s = c.subs.get(msg.sub as number);
        if (s) this.endSub(c, s, 'unsubscribed');
        return;
      }
      case 'input.text':
      case 'input.key':
      case 'input.paste':
      case 'input.submit':
        return this.onInput(c, msg, id);
      default:
        if (id) err('unknown_type');
    }
  }

  private onApi(c: Conn, id: string, method: string, params: Record<string, unknown>): void {
    const ok = (result: Record<string, unknown>) => this.send(c, { t: 'api.response', id, response: { id, result } });
    switch (method) {
      case 'ping':
        return ok({ type: 'pong' });
      case 'agent.list':
        return ok({ type: 'agent_list', agents: this.agentInfos() });
      case 'workspace.list':
        return ok({ type: 'workspace_list', workspaces: this.workspaces });
      case 'tab.list':
        return ok({ type: 'tab_list', tabs: this.tabs });
      case 'agent.get': {
        const a = this.agentInfos().find((x) => x.pane_id === params.target || x.terminal_id === params.target);
        if (!a)
          return this.send(c, {
            t: 'api.response',
            id,
            response: { id, error: { code: 'not_found', message: 'no such agent' } },
          });
        return ok({ type: 'agent', agent: a });
      }
      default:
        return this.send(c, { t: 'error', id, code: 'method_not_allowed', message: method });
    }
  }

  private onSubscribe(c: Conn, msg: Record<string, unknown>, id: string | undefined): void {
    const sub = msg.sub as number;
    const pane = this.pane(String(msg.target));
    const err = (code: string, message?: string) => this.send(c, { t: 'error', id, code, message });
    if (!pane) return err('not_found', `pane ${String(msg.target)} not found`);
    if (c.subs.has(sub)) return err('bad_request', 'sub in use');
    if (c.subs.size >= 4) return err('too_many_subscriptions');
    const mode = msg.mode === 'fit' ? 'fit' : 'observe';
    if (mode === 'fit') {
      if (pane.attachOwner) return err('pane_busy');
      pane.attachOwner = `${sub}`;
      pane.term.resize(clamp(msg.cols as number, 2, 500), clamp(msg.rows as number, 2, 200));
      if (pane.kind === 'tui') this.drawTui(pane);
    }
    const s: Sub = {
      sub,
      pane,
      mode,
      encoder: new SurfaceEncoder(this.opts.shift !== false),
      maxFps: clamp((msg.max_fps as number) ?? 30, 1, 60),
      lastSentAt: 0,
      ackedSeq: 0,
      lastVersion: -1,
      forceFull: true,
      timer: null,
    };
    c.subs.set(sub, s);
    this.send(c, {
      t: 'surface.started',
      id,
      sub,
      pane_id: pane.pane_id,
      terminal_id: pane.terminal_id,
      mode,
      cols: pane.term.cols,
      rows: pane.term.rows,
    });
    this.pump(c, s);
  }

  private endSub(c: Conn, s: Sub, reason: string | null): void {
    if (s.timer) clearTimeout(s.timer);
    c.subs.delete(s.sub);
    if (s.mode === 'fit' && s.pane.attachOwner === `${s.sub}`) {
      s.pane.attachOwner = null;
      s.pane.term.resize(...s.pane.desktopSize);
      if (s.pane.kind === 'tui') this.drawTui(s.pane);
    }
    if (reason) this.send(c, { t: 'surface.ended', sub: s.sub, reason });
  }

  /** Flow control + fps cap (section 10.6). */
  private pump(c: Conn, s: Sub): void {
    if (!c.subs.has(s.sub)) return;
    const t = s.pane.term;
    if (!s.forceFull && t.version === s.lastVersion) return;
    if (s.encoder.seq - s.ackedSeq >= c.maxInFlight) return;
    const now = Date.now();
    const minGap = 1000 / s.maxFps;
    if (now - s.lastSentAt < minGap - 2) {
      if (!s.timer) {
        s.timer = setTimeout(
          () => {
            s.timer = null;
            this.pump(c, s);
          },
          Math.max(0, minGap - 2 - (now - s.lastSentAt)),
        );
      }
      return;
    }
    const msg = s.encoder.encode(
      s.sub,
      t.cells,
      t.cols,
      {
        cursor: {
          x: t.cursorX,
          y: t.cursorY,
          visible: s.pane.kind !== 'stress',
          shape: s.pane.kind === 'shell' ? 5 : 0,
        },
        scroll: { offset: 0, max: t.scrollback.length, viewport: t.rows },
        alt: s.pane.kind === 'tui' || s.pane.kind === 'stress',
      },
      s.forceFull,
    );
    s.forceFull = false;
    s.lastVersion = t.version;
    if (!msg) return;
    s.lastSentAt = now;
    this.send(c, msg as unknown as Record<string, unknown>);
  }

  private onInput(c: Conn, msg: Record<string, unknown>, id: string | undefined): void {
    let pane: MockPane | undefined;
    if (typeof msg.sub === 'number') pane = c.subs.get(msg.sub)?.pane;
    else if (typeof msg.target === 'string') pane = this.pane(msg.target);
    if (!pane) {
      if (id) this.send(c, { t: 'error', id, code: 'not_found' });
      return;
    }
    const t = pane.term;
    const typeText = (text: string) => {
      for (const ch of text) {
        if (ch === '\n' || ch === '\r') this.enter(pane);
        else {
          pane.inputLine += ch;
          t.write(ch);
        }
      }
    };
    switch (msg.t) {
      case 'input.text':
      case 'input.paste':
        typeText(String(msg.text ?? ''));
        break;
      case 'input.submit':
        typeText(String(msg.text ?? ''));
        this.enter(pane);
        break;
      case 'input.key':
        for (const k of (msg.keys as string[]) ?? []) {
          if (k === 'enter') this.enter(pane);
          else if (k === 'backspace') {
            pane.inputLine = pane.inputLine.slice(0, -1);
            t.backspace();
          } else if (k === 'ctrl+c') {
            t.writeLine('^C');
            pane.inputLine = '';
            if (pane.kind === 'shell') t.write('$ ', [2, -1, 1]);
          } else if (k === 'space') typeText(' ');
          else if (k.length === 1) typeText(k);
          else if (
            !/^(esc|tab|shift\+tab|up|down|left|right|home|end|pageup|pagedown|delete|insert|f\d+|(ctrl|alt|shift)\+.+)$/.test(
              k,
            )
          ) {
            if (id) this.send(c, { t: 'error', id, code: 'bad_request', message: `unknown key ${k}` });
            return;
          }
        }
        break;
    }
    if (id) this.send(c, { t: 'input.ok', id });
    // Echo right away instead of waiting for the next pump tick.
    for (const conn of this.conns) for (const s of conn.subs.values()) if (s.pane === pane) this.pump(conn, s);
  }

  private enter(pane: MockPane): void {
    const t = pane.term;
    const cmd = pane.inputLine;
    pane.inputLine = '';
    t.lineFeed();
    if (pane.kind === 'shell') {
      if (cmd.trim()) t.writeLine(`mock: ran "${cmd}"`, [6, -1, 0]);
      t.write('$ ', [2, -1, 1]);
    }
  }
}

function clamp(v: number, lo: number, hi: number): number {
  if (typeof v !== 'number' || Number.isNaN(v)) return lo;
  return Math.min(hi, Math.max(lo, Math.floor(v)));
}

function x0(y: number): number {
  return (y * 37) & 0xff;
}
