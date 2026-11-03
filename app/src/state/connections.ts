// Owns one GatewayClient per paired server, plus pairing.
import Constants from 'expo-constants';
import * as Device from 'expo-device';
import { Platform } from 'react-native';
import {
  ConnStatus,
  decodeServerKey,
  FlockMachine,
  GatewayClient,
  HANDSHAKE_FAILED,
  NotifyMessage,
  PairingPayload,
  parseHostEntry,
  WebSocketLike,
} from '../protocol';
import { DEMO_DEVICE_ID, DEMO_HUB_ID, DEMO_TOKEN, DemoFlock } from '../demo/flock';
import {
  deleteServer,
  loadDemoFlag,
  loadServers,
  repairReason,
  saveDemoFlag,
  saveServer,
  StoredServer,
} from './storage';

const createSocket = (url: string, protocols: string[]) => new WebSocket(url, protocols) as unknown as WebSocketLike;

export function appInfo() {
  return {
    version: Constants.expoConfig?.version ?? '0.0.0',
    platform: Platform.OS,
    build: Constants.nativeBuildVersion ?? undefined,
  };
}

export function deviceName(): string {
  if (Platform.OS === 'web') return browserName();
  return (Device.deviceName || Device.modelName || `${Platform.OS} device`).slice(0, 64);
}

/** "Chrome on Linux" and the like, from the user agent. */
function browserName(): string {
  const ua = globalThis.navigator?.userAgent ?? '';
  const browser = /Edg\//.test(ua)
    ? 'Edge'
    : /Firefox\//.test(ua)
      ? 'Firefox'
      : /Chrome\//.test(ua)
        ? 'Chrome'
        : /Safari\//.test(ua)
          ? 'Safari'
          : 'Browser';
  const os = /Windows/.test(ua)
    ? 'Windows'
    : /Mac OS X/.test(ua)
      ? 'macOS'
      : /Android/.test(ua)
        ? 'Android'
        : /Linux/.test(ua)
          ? 'Linux'
          : '';
  return os ? `${browser} on ${os}` : browser;
}

export interface InbandNotify extends NotifyMessage {
  server_id: string;
  server_name: string;
}

type Listener = () => void;

class Connections {
  private servers: StoredServer[] = [];
  private clients = new Map<string, GatewayClient>();
  private listeners = new Set<Listener>();
  private notifyListeners = new Set<(n: InbandNotify) => void>();
  private loaded: Promise<void> | null = null;
  private foreground = true;
  private pushToken: string | null = null;
  private pushRegistered = new Set<string>();
  private demo: DemoFlock | null = null;
  version = 0;

  load(): Promise<void> {
    if (!this.loaded) {
      this.loaded = Promise.all([loadServers(), loadDemoFlag()]).then(([s, demo]) => {
        this.servers = demo ? [...s, this.demoHub()] : s;
        this.changed();
      });
    }
    return this.loaded;
  }

  /** Demo mode: a made-up flock that runs in the app, for trying it without a server. */
  get demoOn(): boolean {
    return this.servers.some((s) => s.server_id === DEMO_HUB_ID);
  }

  async startDemo(): Promise<void> {
    if (this.demoOn) return;
    this.servers = [...this.servers, this.demoHub()];
    this.changed();
    await saveDemoFlag(true);
  }

  async stopDemo(): Promise<void> {
    if (this.demoOn) await this.remove(DEMO_HUB_ID);
    else await saveDemoFlag(false);
  }

  private demoHub(): StoredServer {
    if (!this.demo) this.demo = new DemoFlock();
    return {
      server_id: DEMO_HUB_ID,
      name: 'studio',
      hosts: ['demo'],
      port: 0,
      pk: this.demo.pk,
      device_id: DEMO_DEVICE_ID,
      token: DEMO_TOKEN,
      pairedAt: Date.now(),
      demo: true,
    };
  }

  // Demo machines live in memory only.
  private async store(s: StoredServer): Promise<void> {
    if (!s.demo) await saveServer(s);
  }

  private async unstore(s: StoredServer): Promise<void> {
    if (!s.demo) await deleteServer(s.server_id);
  }

  subscribe = (cb: Listener): (() => void) => {
    this.listeners.add(cb);
    return () => this.listeners.delete(cb);
  };

  getVersion = (): number => this.version;

  private changed(): void {
    this.version++;
    for (const cb of Array.from(this.listeners)) cb();
  }

  onNotify(cb: (n: InbandNotify) => void): () => void {
    this.notifyListeners.add(cb);
    return () => this.notifyListeners.delete(cb);
  }

  list(): StoredServer[] {
    return this.servers;
  }

  server(serverId: string): StoredServer | undefined {
    return this.servers.find((s) => s.server_id === serverId);
  }

  /** Connected (or connecting) client for a server; created on first use. */
  client(serverId: string): GatewayClient | null {
    const existing = this.clients.get(serverId);
    if (existing) return existing;
    const s = this.server(serverId);
    if (!s || repairReason(s)) return null;
    const serverKey = decodeServerKey(s.pk);
    if (!serverKey) return null;
    const demo = s.demo ? this.demo : null;
    if (s.demo && !demo) return null;
    const client = new GatewayClient({
      hosts: s.hosts.map((h) => parseHostEntry(h, s.port)),
      // A flock machine answers through the paired server, with its ID.
      sid: s.via ?? s.server_id,
      machine: s.machine,
      serverKey,
      credentials: { kind: 'auth', device_id: s.device_id, token: s.token },
      app: appInfo(),
      createSocket: demo ? demo.createSocket : createSocket,
      preferredHost: s.lastHost,
      log: __DEV__ ? (m) => console.log(`[flockr ${s.name}] ${m}`) : undefined,
    });
    client.on('welcome', (w) => {
      const idx = client.currentHostIndex;
      const name = w.server.name || s.name;
      if (idx !== s.lastHost || name !== s.name) void this.update(serverId, { lastHost: idx, name });
      if (!s.via) void this.syncMachines(serverId, w.machines ?? []);
      void this.registerPush(serverId, client);
    });
    client.on('fatal', (f) => {
      if (f.tokenDead) void this.update(serverId, { needsRepair: true });
      // Keep the token: the user may fix the address instead of re-pairing.
      else if (f.error === HANDSHAKE_FAILED) void this.update(serverId, { keyMismatch: true });
      this.clients.delete(serverId);
      this.changed();
    });
    client.on('notify', (n) => {
      for (const cb of Array.from(this.notifyListeners)) cb({ ...n, server_id: s.server_id, server_name: s.name });
    });
    client.setForeground(this.foreground);
    this.clients.set(serverId, client);
    client.connect();
    return client;
  }

  private async update(serverId: string, patch: Partial<StoredServer>): Promise<void> {
    const i = this.servers.findIndex((s) => s.server_id === serverId);
    if (i < 0) return;
    const next = { ...this.servers[i], ...patch };
    this.servers = this.servers.map((s, j) => (j === i ? next : s));
    await this.store(next);
    this.changed();
  }

  setForeground(fg: boolean): void {
    this.foreground = fg;
    for (const c of this.clients.values()) {
      c.setForeground(fg);
      if (fg) c.reconnectNow();
    }
  }

  setPushToken(token: string | null): void {
    if (token === this.pushToken) return;
    this.pushToken = token;
    this.pushRegistered.clear();
    for (const [id, c] of this.clients) void this.registerPush(id, c);
  }

  get currentPushToken(): string | null {
    return this.pushToken;
  }

  private async registerPush(serverId: string, client: GatewayClient): Promise<void> {
    const token = this.pushToken;
    if (this.server(serverId)?.demo) return;
    if (!token || !client.ready || !client.hasFeature('push.expo')) return;
    const key = `${serverId}:${client.welcome?.server.boot_id ?? ''}`;
    if (this.pushRegistered.has(key)) return;
    try {
      await client.registerPush(token, Platform.OS);
      this.pushRegistered.add(key);
    } catch (e) {
      if (__DEV__) console.log(`push.register failed: ${String(e)}`);
    }
  }

  /**
   * Keeps one entry per other machine in a paired server's flock. They share
   * the server's pairing and connect through it.
   */
  private async syncMachines(serverId: string, machines: FlockMachine[]): Promise<void> {
    const hub = this.server(serverId);
    if (!hub) return;
    const wanted: StoredServer[] = machines
      .filter((m) => !m.this)
      .map((m) => {
        const id = `${hub.server_id}~${m.id}`;
        const existing = this.server(id);
        return {
          server_id: id,
          name: m.name,
          hosts: hub.hosts,
          port: hub.port,
          pk: hub.pk,
          device_id: hub.device_id,
          token: hub.token,
          lastHost: existing?.lastHost ?? hub.lastHost,
          pairedAt: existing?.pairedAt ?? Date.now(),
          machine: m.id,
          via: hub.server_id,
          ...(hub.demo ? { demo: true } : {}),
        };
      });
    const wantedIds = new Set(wanted.map((s) => s.server_id));
    const stale = this.servers.filter((s) => s.via === hub.server_id && !wantedIds.has(s.server_id));
    const changed = wanted.filter((w) => {
      const existing = this.server(w.server_id);
      return !existing || JSON.stringify(existing) !== JSON.stringify(w);
    });
    if (stale.length === 0 && changed.length === 0) return;
    for (const s of stale) {
      this.clients.get(s.server_id)?.close();
      this.clients.delete(s.server_id);
      await this.unstore(s);
    }
    for (const s of changed) {
      this.clients.get(s.server_id)?.close();
      this.clients.delete(s.server_id);
      await this.store(s);
    }
    const byId = new Map(changed.map((s) => [s.server_id, s]));
    const staleIds = new Set(stale.map((s) => s.server_id));
    this.servers = [
      ...this.servers.filter((s) => !staleIds.has(s.server_id)).map((s) => byId.get(s.server_id) ?? s),
      ...changed.filter((s) => !this.servers.some((e) => e.server_id === s.server_id)),
    ];
    this.changed();
  }

  /** Pair with a scanned/pasted payload. Resolves with the stored server. */
  async pair(payload: PairingPayload): Promise<StoredServer> {
    const serverKey = decodeServerKey(payload.pk);
    if (!serverKey) throw new Error('pairing payload has no valid server key');
    const client = new GatewayClient({
      hosts: payload.hosts.map((h) => parseHostEntry(h, payload.port)),
      sid: payload.sid,
      serverKey,
      credentials: { kind: 'pair', code: payload.code, device_name: deviceName() },
      app: appInfo(),
      createSocket,
      reconnect: false,
      pingIntervalMs: 0,
    });
    let paired: { device_id: string; token: string } | null = null;
    client.on('paired', (d) => {
      paired = { device_id: d.device_id, token: d.token };
    });
    client.connect();
    try {
      const welcome = await client.waitReady(20000);
      const got = paired as { device_id: string; token: string } | null;
      if (!got) throw new Error('server did not issue a device token');
      const server: StoredServer = {
        server_id: payload.sid,
        name: welcome.server.name || payload.name,
        hosts: payload.hosts,
        port: payload.port,
        pk: payload.pk,
        device_id: got.device_id,
        token: got.token,
        lastHost: client.currentHostIndex,
        pairedAt: Date.now(),
      };
      this.clients.get(server.server_id)?.close();
      this.clients.delete(server.server_id);
      await saveServer(server);
      this.servers = [...this.servers.filter((s) => s.server_id !== server.server_id), server];
      this.changed();
      return server;
    } finally {
      client.close();
    }
  }

  /** Forget this phone on the server (if reachable) and locally, with its flock machines. */
  async remove(serverId: string): Promise<void> {
    for (const machine of this.servers.filter((s) => s.via === serverId)) {
      this.clients.get(machine.server_id)?.close();
      this.clients.delete(machine.server_id);
      await this.unstore(machine);
    }
    this.servers = this.servers.filter((s) => s.via !== serverId);
    const c = this.clients.get(serverId);
    const server = this.server(serverId);
    // A flock machine shares its server's pairing; removing it only hides it
    // until the server lists it again.
    if (c?.ready && server && !server.via && !server.demo) {
      try {
        await c.forget();
      } catch {
        // remove locally anyway
      }
    }
    c?.close();
    this.clients.delete(serverId);
    if (server) await this.unstore(server);
    this.servers = this.servers.filter((s) => s.server_id !== serverId);
    if (serverId === DEMO_HUB_ID) {
      this.demo?.stop();
      this.demo = null;
      await saveDemoFlag(false);
    }
    this.changed();
  }
}

export const connections = new Connections();

export type { ConnStatus };
