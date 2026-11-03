// Demo mode: a made-up flock of two machines that runs inside the app. The
// app's real GatewayClient talks to it through an in-memory socket, with the
// same Noise channel and messages as a real server, so every screen works
// unchanged and nothing goes over the network. Pure TypeScript: it is
// bundled into the app, so no Node modules here.
import type { WebSocketLike } from '../protocol/client';
import {
  CipherState,
  FrameError,
  generateKeyPair,
  HANDSHAKE_MSG_LEN,
  KeyPair,
  MessageReader,
  NoiseResponder,
  sealMessage,
} from '../protocol/noise';
import { base64UrlEncode } from '../protocol/pairing';
import type { AgentInfo, AgentStatus, FlockMachine, TabInfo, WorkspaceInfo } from '../protocol/types';
import { SurfaceEncoder } from '../../mock/encoder';
import {
  apiGetsMessage,
  COUPON_QUESTION,
  DemoPane,
  DESKTOP_SIZE,
  MachineDef,
  PaneHost,
  sceneMachines,
  storefrontGetsReply,
} from './scene';

/** Server ID of the demo hub. Its other machine is `${DEMO_HUB_ID}~${member id}`. */
export const DEMO_HUB_ID = 'demo-studio';
export const DEMO_DEVICE_ID = 'demo-device';
export const DEMO_TOKEN = 'demo-token';

type ApproveMode = 'other-projects' | 'never' | 'always';
const APPROVE_MODES: ApproveMode[] = ['other-projects', 'never', 'always'];

interface Party {
  member: string;
  machine: string;
  agent?: string | null;
}

interface Pending {
  id: string;
  conversation: string;
  from: Party;
  to: Party;
  body: string;
  sent_at: number;
}

interface Machine {
  def: MachineDef;
  panes: DemoPane[];
  pending: Pending[];
  approve: ApproveMode;
}

interface Sub {
  sub: number;
  pane: DemoPane;
  mode: 'observe' | 'fit';
  encoder: SurfaceEncoder;
  ackedSeq: number;
  lastVersion: number;
  forceFull: boolean;
}

const STATUS_ORDER: AgentStatus[] = ['blocked', 'done', 'working', 'idle', 'unknown'];

function worst(statuses: AgentStatus[]): AgentStatus {
  for (const s of STATUS_ORDER) if (statuses.includes(s)) return s;
  return 'idle';
}

/** API key names ("Enter", "Escape", "Up") as the demo panes name them. */
function demoKey(key: string): string {
  const k = key.trim();
  if (k.length === 1) return k;
  const lower = k.toLowerCase();
  return lower === 'escape' ? 'esc' : lower;
}

const clamp = (v: unknown, lo: number, hi: number) =>
  typeof v === 'number' && Number.isFinite(v) ? Math.min(hi, Math.max(lo, Math.floor(v))) : lo;

/** The in-memory socket the app's client uses for a demo machine. */
class DemoSocket implements WebSocketLike {
  readyState = 0;
  binaryType?: string;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: { code: number; reason: string }) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  private conn: Conn | null = null;

  constructor(private readonly flock: DemoFlock) {
    setTimeout(() => {
      if (this.readyState !== 0) return;
      this.readyState = 1;
      this.conn = flock.accept(this);
      this.onopen?.({});
    }, 0);
  }

  send(data: Uint8Array): void {
    if (this.readyState !== 1) return;
    const copy = data.slice();
    setTimeout(() => this.conn?.receive(copy), 0);
  }

  close(code = 1000, reason = ''): void {
    if (this.readyState >= 2) return;
    this.readyState = 3;
    this.conn?.gone();
    this.conn = null;
    setTimeout(() => this.onclose?.({ code, reason }), 0);
  }

  /** Server side: one binary frame to the app. */
  deliver(bytes: Uint8Array): void {
    if (this.readyState !== 1) return;
    setTimeout(() => {
      if (this.readyState === 1) this.onmessage?.({ data: bytes });
    }, 0);
  }

  /** Server side: close with a code. */
  shut(code: number, reason: string): void {
    if (this.readyState >= 2) return;
    this.readyState = 3;
    this.conn?.gone();
    this.conn = null;
    setTimeout(() => this.onclose?.({ code, reason }), 0);
  }
}

/** One app connection: Noise, then the session for one machine. */
class Conn {
  private noise: NoiseResponder | null;
  private tx: CipherState | null = null;
  private reader: MessageReader | null = null;
  machine: Machine | null = null;
  subs = new Map<number, Sub>();
  foreground = false;
  watchId: string | null = null;
  lastAgents = '';
  rev = 0;
  maxInFlight = 2;
  closed = false;

  constructor(
    private readonly flock: DemoFlock,
    readonly socket: DemoSocket,
    key: KeyPair,
  ) {
    this.noise = new NoiseResponder(key);
  }

  receive(bytes: Uint8Array): void {
    if (this.closed) return;
    if (this.noise) {
      try {
        if (bytes.length !== HANDSHAKE_MSG_LEN) throw new Error('bad handshake');
        this.noise.readMessage1(bytes);
        const { message, ciphers } = this.noise.writeMessage2();
        this.noise = null;
        this.socket.deliver(message);
        this.tx = ciphers.send;
        this.reader = new MessageReader(ciphers.recv, 1024 * 1024);
      } catch {
        this.socket.shut(4006, 'handshake_failed');
      }
      return;
    }
    if (!this.reader) return;
    let text: string | null;
    try {
      text = this.reader.push(bytes);
    } catch (e) {
      this.socket.shut(e instanceof FrameError ? e.closeCode : 4000, 'bad frame');
      return;
    }
    if (text === null) return;
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(text) as Record<string, unknown>;
    } catch {
      this.socket.shut(4000, 'bad json');
      return;
    }
    this.flock.onMessage(this, msg);
  }

  send(msg: Record<string, unknown>): void {
    if (this.closed || !this.tx) return;
    for (const frame of sealMessage(this.tx, JSON.stringify(msg))) this.socket.deliver(frame);
  }

  gone(): void {
    if (this.closed) return;
    this.closed = true;
    this.flock.dropped(this);
  }
}

export interface DemoFlockOptions {
  /** Run the clock and spinners on a timer. Tests turn it off and call advance(). */
  ticker?: boolean;
}

export class DemoFlock {
  readonly key = generateKeyPair();
  readonly bootId = `demo-${Math.floor(Math.random() * 1e9).toString(36)}`;
  readonly machines: Machine[];
  private readonly conns = new Set<Conn>();
  private timer: ReturnType<typeof setInterval> | null = null;
  private queue: { at: number; fn: () => void }[] = [];
  private offset = 0;

  constructor(private readonly opts: DemoFlockOptions = {}) {
    const machines: Machine[] = [];
    const hostFor = (index: number): PaneHost => ({
      now: () => this.now(),
      later: (ms, fn) => this.later(ms, fn),
      statusChanged: (pane) => this.statusChanged(machines[index], pane),
    });
    // Each pane reports status changes to its own machine, looked up lazily.
    for (const def of sceneMachines(hostFor(0))) {
      const index = machines.length;
      machines.push({
        def,
        panes: def.panes.map((p) => new DemoPane(p, hostFor(index))),
        pending: [],
        approve: 'other-projects',
      });
    }
    this.machines = machines;
    const [studio, buildBox] = machines;
    studio.pending.push({
      id: 'msg_5c2e91',
      conversation: 'conv_save10',
      from: { member: buildBox.def.id, machine: buildBox.def.name, agent: 'storefront-ui' },
      to: { member: studio.def.id, machine: studio.def.name, agent: 'api' },
      body: COUPON_QUESTION,
      sent_at: Math.floor(this.now() / 1000) - 4 * 60,
    });
  }

  /** The hub's public key as a pairing payload carries it. */
  get pk(): string {
    return base64UrlEncode(this.key.publicKey);
  }

  /** Member IDs and names of the flock, hub first. */
  get members(): FlockMachine[] {
    return this.machines.map((m, i) => ({ id: m.def.id, name: m.def.name, this: i === 0, online: true }));
  }

  /** Socket factory for the app's client. */
  createSocket = (): WebSocketLike => new DemoSocket(this);

  now(): number {
    return Date.now() + this.offset;
  }

  /** Move demo time forward (tests), running what came due. */
  advance(ms: number): void {
    this.offset += ms;
    this.step();
  }

  stop(): void {
    for (const c of Array.from(this.conns)) c.socket.shut(4009, 'server_shutdown');
    this.conns.clear();
    this.stopTicker();
    this.queue = [];
  }

  pane(machineId: string, paneId: string): DemoPane | undefined {
    return this.machines.find((m) => m.def.id === machineId)?.panes.find((p) => p.id === paneId);
  }

  // --- time ---

  private later(ms: number, fn: () => void): void {
    this.queue.push({ at: this.now() + ms, fn });
    this.queue.sort((a, b) => a.at - b.at);
  }

  private step(): void {
    const now = this.now();
    while (this.queue.length && this.queue[0].at <= now) {
      const next = this.queue.shift();
      next?.fn();
    }
    for (const m of this.machines) for (const p of m.panes) p.tick();
    this.pumpAll();
  }

  private startTicker(): void {
    if (this.timer || this.opts.ticker === false) return;
    this.timer = setInterval(() => this.step(), 200);
  }

  private stopTicker(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
  }

  // --- connections ---

  /** @internal */
  accept(socket: DemoSocket): Conn {
    const conn = new Conn(this, socket, this.key);
    this.conns.add(conn);
    this.startTicker();
    return conn;
  }

  /** @internal */
  dropped(c: Conn): void {
    for (const s of Array.from(c.subs.values())) this.endSub(c, s, null);
    this.conns.delete(c);
    if (this.conns.size === 0) this.stopTicker();
  }

  /** @internal */
  onMessage(c: Conn, msg: Record<string, unknown>): void {
    if (typeof msg.t !== 'string') return c.socket.shut(4000, 'missing t');
    if (!c.machine) {
      if (msg.t !== 'hello') return c.socket.shut(4000, 'expected hello');
      return this.hello(c, msg);
    }
    const id = typeof msg.id === 'string' ? msg.id : undefined;
    const err = (code: string, message?: string) => c.send({ t: 'error', ...(id ? { id } : {}), code, message });
    switch (msg.t) {
      case 'ping':
        return c.send({ t: 'pong', ts: msg.ts });
      case 'app.state':
        c.foreground = !!msg.foreground;
        return;
      case 'agents.watch':
        c.watchId = id ?? null;
        return this.sendAgents(c, true);
      case 'agents.unwatch':
        if (c.watchId === id) c.watchId = null;
        return c.send({ t: 'api.end', id });
      case 'api.request':
        return this.api(c, id ?? '', String(msg.method), (msg.params ?? {}) as Record<string, unknown>);
      case 'api.cancel':
        return c.send({ t: 'api.end', id });
      case 'push.register':
      case 'push.unregister':
        // Demo machines never register for push.
        return c.send({ t: 'push.ok', id });
      case 'device.forget':
        c.send({ t: 'device.ok', id });
        return c.socket.shut(1000, 'forgotten');
      case 'surface.subscribe':
        return this.subscribe(c, msg, id);
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
        s.pane.resize(clamp(msg.cols, 20, 500), clamp(msg.rows, 5, 200));
        return this.pumpPane(s.pane);
      }
      case 'surface.scroll':
      case 'surface.wheel':
        return;
      case 'surface.history': {
        const s = c.subs.get(msg.sub as number);
        if (!s) return err('not_found');
        const lines = s.pane.history().slice(-clamp(msg.lines ?? 500, 1, 10000));
        return c.send({ t: 'surface.history.result', id, sub: s.sub, lines });
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
        return this.input(c, msg, id);
      default:
        if (id) err('unknown_type');
    }
  }

  private hello(c: Conn, msg: Record<string, unknown>): void {
    const auth = msg.auth as { device_id?: string } | undefined;
    if (!auth || auth.device_id !== DEMO_DEVICE_ID) {
      c.send({ t: 'error', code: 'unauthorized' });
      return c.socket.shut(4001, 'unauthorized');
    }
    const wanted = typeof msg.machine === 'string' ? msg.machine : this.machines[0].def.id;
    const machine = this.machines.find((m) => m.def.id === wanted);
    if (!machine) return c.socket.shut(4000, 'unknown machine');
    c.machine = machine;
    c.maxInFlight = clamp(msg.max_in_flight ?? 2, 1, 8);
    const relayed = machine !== this.machines[0];
    c.send({
      t: 'welcome',
      version: 2,
      server: { name: machine.def.name, server_id: DEMO_HUB_ID, flockr_version: 'demo', boot_id: this.bootId },
      device: { device_id: DEMO_DEVICE_ID, name: 'demo' },
      features: ['api', 'agents.watch', 'surface', 'surface.history', 'surface.shift', 'input', 'messages'],
      methods: [
        'ping',
        'agent.list',
        'agent.get',
        'workspace.list',
        'tab.list',
        'pane.list',
        'pane.mark_read',
        'agent.read',
        'pane.read',
        'agent.send_keys',
        'pane.send_keys',
        'message.pending',
        'message.approve',
        'message.deny',
        'message.settings',
        'message.settings.set',
      ],
      limits: { max_in_flight: c.maxInFlight, max_subscriptions: 4, max_cols: 500, max_rows: 200 },
      ...(relayed ? { machine: machine.def.id } : { machines: this.members }),
    });
  }

  // --- agents ---

  private agentInfos(m: Machine): AgentInfo[] {
    return m.panes
      .filter((p) => p.isAgent)
      .map((p) => ({
        terminal_id: `term_${p.id}`,
        pane_id: p.id,
        workspace_id: p.def.workspace_id,
        tab_id: p.def.tab_id,
        name: p.def.name,
        agent: p.def.kind,
        display_agent: p.def.name,
        title: p.def.title,
        terminal_title_stripped: p.def.title,
        agent_status: p.status,
        state_change_seq: p.stateChangeSeq,
        revision: p.stateChangeSeq,
        focused: false,
        cwd: p.def.cwd,
        ...(p.def.branch ? { branch: p.def.branch } : {}),
      }));
  }

  private workspaces(m: Machine): WorkspaceInfo[] {
    return m.def.workspaces.map((w, i) => {
      const panes = m.panes.filter((p) => p.def.workspace_id === w.workspace_id);
      return {
        workspace_id: w.workspace_id,
        number: i + 1,
        label: w.label,
        focused: i === 0,
        agent_status: worst(panes.filter((p) => p.isAgent).map((p) => p.status)),
        active_tab_id: w.tabs[0]?.tab_id,
        pane_count: panes.length,
        tab_count: w.tabs.length,
      };
    });
  }

  private tabs(m: Machine): TabInfo[] {
    return m.def.workspaces.flatMap((w) =>
      w.tabs.map((t, i) => {
        const panes = m.panes.filter((p) => p.def.tab_id === t.tab_id && p.def.workspace_id === w.workspace_id);
        return {
          tab_id: t.tab_id,
          workspace_id: w.workspace_id,
          number: i + 1,
          label: t.label,
          focused: i === 0,
          pane_count: panes.length,
          agent_status: worst(panes.filter((p) => p.isAgent).map((p) => p.status)),
        };
      }),
    );
  }

  private sendAgents(c: Conn, force: boolean): void {
    const m = c.machine;
    if (!c.watchId || !m) return;
    const payload = { agents: this.agentInfos(m), workspaces: this.workspaces(m), tabs: this.tabs(m) };
    const key = JSON.stringify(payload);
    if (!force && key === c.lastAgents) return;
    c.lastAgents = key;
    c.rev++;
    c.send({ t: 'agents', id: c.watchId, rev: c.rev, ...payload });
  }

  private statusChanged(m: Machine, pane: DemoPane): void {
    for (const c of this.conns) {
      if (c.machine !== m) continue;
      this.sendAgents(c, false);
      if ((pane.status === 'done' || pane.status === 'blocked') && c.foreground) {
        const ws = m.def.workspaces.find((w) => w.workspace_id === pane.def.workspace_id);
        c.send({
          t: 'notify',
          kind: pane.status,
          pane_id: pane.id,
          workspace_id: pane.def.workspace_id,
          tab_id: pane.def.tab_id,
          terminal_id: `term_${pane.id}`,
          title: pane.status === 'done' ? `${pane.def.name} finished` : `${pane.def.name} is blocked`,
          body: `${ws?.label ?? ''} · ${m.def.name}`,
        });
      }
    }
  }

  // --- API ---

  private api(c: Conn, id: string, method: string, params: Record<string, unknown>): void {
    const m = c.machine;
    if (!m) return;
    const ok = (result: Record<string, unknown>) => c.send({ t: 'api.response', id, response: { id, result } });
    const fail = (code: string, message: string) =>
      c.send({ t: 'api.response', id, response: { id, error: { code, message } } });
    switch (method) {
      case 'ping':
        return ok({ type: 'pong' });
      case 'agent.list':
        return ok({ type: 'agent_list', agents: this.agentInfos(m) });
      case 'agent.get': {
        const a = this.agentInfos(m).find((x) => x.pane_id === params.target || x.terminal_id === params.target);
        return a ? ok({ type: 'agent', agent: a }) : fail('not_found', 'no such agent');
      }
      case 'workspace.list':
        return ok({ type: 'workspace_list', workspaces: this.workspaces(m) });
      case 'tab.list':
        return ok({ type: 'tab_list', tabs: this.tabs(m) });
      case 'pane.list':
        return ok({
          type: 'pane_list',
          panes: m.panes.map((p) => ({
            pane_id: p.id,
            workspace_id: p.def.workspace_id,
            tab_id: p.def.tab_id,
            ...(p.isAgent ? { agent: p.def.kind } : {}),
            cwd: p.def.cwd,
            terminal_title_stripped: p.def.title,
          })),
        });
      case 'pane.mark_read': {
        const p = m.panes.find((x) => x.id === params.pane_id);
        if (!p) return fail('pane_not_found', `pane not found: ${String(params.pane_id)}`);
        const seq = typeof params.completion_seq === 'number' ? params.completion_seq : undefined;
        // Like the server: a done agent turns idle without a new state change.
        if (p.status === 'done' && (seq === undefined || p.stateChangeSeq <= seq)) {
          p.status = 'idle';
          for (const conn of this.conns) if (conn.machine === m) this.sendAgents(conn, false);
        }
        return ok({ type: 'ok' });
      }
      case 'agent.read':
      case 'pane.read': {
        const target = method === 'agent.read' ? params.target : params.pane_id;
        const p = m.panes.find((x) => x.id === target || `term_${x.id}` === target);
        if (!p || (method === 'agent.read' && !p.isAgent)) return fail('not_found', `not found: ${String(target)}`);
        // Every source reads the visible screen; `lines` keeps the bottom rows.
        const rows = p.screen();
        const lines = typeof params.lines === 'number' && params.lines > 0 ? rows.slice(-params.lines) : rows;
        return ok({
          type: 'pane_read',
          read: {
            pane_id: p.id,
            workspace_id: p.def.workspace_id,
            tab_id: p.def.tab_id,
            source: params.source ?? 'recent',
            format: 'text',
            text: lines.join('\n'),
            revision: 0,
            truncated: false,
          },
        });
      }
      case 'agent.send_keys':
      case 'pane.send_keys': {
        const target = method === 'agent.send_keys' ? params.target : params.pane_id;
        const p = m.panes.find((x) => x.id === target || `term_${x.id}` === target);
        if (!p || (method === 'agent.send_keys' && !p.isAgent))
          return fail('not_found', `not found: ${String(target)}`);
        const keys = Array.isArray(params.keys) ? params.keys.map(String) : [];
        for (const k of keys) p.key(demoKey(k));
        this.pumpPane(p);
        return ok({ type: 'ok' });
      }
      case 'message.pending':
        return ok({ pending: m.pending });
      case 'message.approve':
      case 'message.deny': {
        const i = m.pending.findIndex((x) => x.id === params.id);
        if (i < 0) return fail('not_found', `no pending message ${String(params.id)}`);
        const [msg] = m.pending.splice(i, 1);
        if (method === 'message.approve') {
          this.deliver(msg);
          return ok({ id: msg.id, to: `${msg.to.machine}:${msg.to.agent}` });
        }
        this.later(600, () =>
          this.agentPane(msg.from)?.say({
            segs: [['  ⎿  A person denied the message to studio:api.', [16777216 + 0xe8707d, -1, 0]]],
            indent: 5,
          }),
        );
        return ok({ id: msg.id });
      }
      case 'message.settings':
        return ok({ approve_messages: m.approve });
      case 'message.settings.set': {
        const mode = params.approve_messages as ApproveMode;
        if (!APPROVE_MODES.includes(mode))
          return fail('bad_request', 'approve_messages must be other-projects, never or always');
        m.approve = mode;
        return ok({ approve_messages: mode });
      }
      default:
        return fail('method_not_allowed', `${method} is not available on a demo machine`);
    }
  }

  private agentPane(party: Party): DemoPane | undefined {
    const m = this.machines.find((x) => x.def.id === party.member);
    return m?.panes.find((p) => p.def.name === party.agent);
  }

  /** An approved message: the recipient reads it and replies. */
  private deliver(msg: Pending): void {
    const to = this.agentPane(msg.to);
    const from = this.agentPane(msg.from);
    this.later(800, () => to?.say(...apiGetsMessage()));
    this.later(2400, () => from?.say(...storefrontGetsReply()));
  }

  // --- surfaces ---

  private subscribe(c: Conn, msg: Record<string, unknown>, id: string | undefined): void {
    const m = c.machine;
    const sub = msg.sub as number;
    const err = (code: string, message?: string) => c.send({ t: 'error', id, code, message });
    const pane = m?.panes.find((p) => p.id === msg.target || `term_${p.id}` === msg.target);
    if (!pane) return err('not_found', `pane ${String(msg.target)} not found`);
    if (c.subs.has(sub)) return err('bad_request', 'sub in use');
    if (c.subs.size >= 4) return err('too_many_subscriptions');
    const mode = msg.mode === 'fit' ? 'fit' : 'observe';
    if (mode === 'fit') pane.resize(clamp(msg.cols, 20, 500), clamp(msg.rows, 5, 200));
    const s: Sub = {
      sub,
      pane,
      mode,
      encoder: new SurfaceEncoder(true),
      ackedSeq: 0,
      lastVersion: -1,
      forceFull: true,
    };
    c.subs.set(sub, s);
    c.send({
      t: 'surface.started',
      id,
      sub,
      pane_id: pane.id,
      terminal_id: `term_${pane.id}`,
      mode,
      cols: pane.term.cols,
      rows: pane.term.rows,
    });
    this.pumpPane(pane);
  }

  private endSub(c: Conn, s: Sub, reason: string | null): void {
    c.subs.delete(s.sub);
    if (s.mode === 'fit') {
      const stillFit = Array.from(this.conns).some((x) =>
        Array.from(x.subs.values()).some((o) => o.pane === s.pane && o.mode === 'fit'),
      );
      if (!stillFit) s.pane.resize(...DESKTOP_SIZE);
    }
    if (reason) c.send({ t: 'surface.ended', sub: s.sub, reason });
  }

  private pumpAll(): void {
    for (const c of this.conns) for (const s of c.subs.values()) this.pump(c, s);
  }

  private pumpPane(pane: DemoPane): void {
    for (const c of this.conns) for (const s of c.subs.values()) if (s.pane === pane) this.pump(c, s);
  }

  private pump(c: Conn, s: Sub): void {
    if (!c.subs.has(s.sub)) return;
    const t = s.pane.term;
    if (!s.forceFull && t.version === s.lastVersion) return;
    if (s.encoder.seq - s.ackedSeq >= c.maxInFlight) return;
    const msg = s.encoder.encode(
      s.sub,
      t.cells,
      t.cols,
      { cursor: s.pane.cursor, scroll: { offset: 0, max: 0, viewport: t.rows }, alt: false },
      s.forceFull,
    );
    s.forceFull = false;
    s.lastVersion = t.version;
    if (msg) c.send(msg as unknown as Record<string, unknown>);
  }

  private input(c: Conn, msg: Record<string, unknown>, id: string | undefined): void {
    const pane = typeof msg.sub === 'number' ? c.subs.get(msg.sub)?.pane : undefined;
    if (!pane) {
      if (id) c.send({ t: 'error', id, code: 'not_found' });
      return;
    }
    const text = String(msg.text ?? '');
    switch (msg.t) {
      case 'input.text':
      case 'input.paste':
        pane.text(text);
        break;
      case 'input.submit':
        pane.text(text);
        pane.key('enter');
        break;
      case 'input.key':
        for (const k of (msg.keys as string[]) ?? []) pane.key(String(k));
        break;
    }
    if (id) c.send({ t: 'input.ok', id });
    this.pumpPane(pane);
  }
}
