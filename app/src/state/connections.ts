// Owns one GatewayClient per paired server, plus pairing.
import Constants from 'expo-constants';
import * as Device from 'expo-device';
import { Platform } from 'react-native';
import { ConnStatus, GatewayClient, NotifyMessage, PairingPayload, parseHostEntry, WebSocketLike } from '../protocol';
import { deleteServer, loadServers, saveServer, StoredServer } from './storage';

const createSocket = (url: string, protocols: string[]) => new WebSocket(url, protocols) as unknown as WebSocketLike;

export function appInfo() {
  return {
    version: Constants.expoConfig?.version ?? '0.0.0',
    platform: Platform.OS,
    build: Constants.nativeBuildVersion ?? undefined,
  };
}

export function deviceName(): string {
  return (Device.deviceName || Device.modelName || `${Platform.OS} device`).slice(0, 64);
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
  version = 0;

  load(): Promise<void> {
    if (!this.loaded) {
      this.loaded = loadServers().then((s) => {
        this.servers = s;
        this.changed();
      });
    }
    return this.loaded;
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
    if (!s || s.needsRepair) return null;
    const client = new GatewayClient({
      hosts: s.hosts.map((h) => parseHostEntry(h, s.port)),
      sid: s.server_id,
      credentials: { kind: 'auth', device_id: s.device_id, token: s.token },
      app: appInfo(),
      createSocket,
      preferredHost: s.lastHost,
      log: __DEV__ ? (m) => console.log(`[flockr ${s.name}] ${m}`) : undefined,
    });
    client.on('welcome', (w) => {
      const idx = client.currentHostIndex;
      const name = w.server.name || s.name;
      if (idx !== s.lastHost || name !== s.name) void this.update(serverId, { lastHost: idx, name });
      void this.registerPush(serverId, client);
    });
    client.on('fatal', (f) => {
      if (f.tokenDead) void this.update(serverId, { needsRepair: true });
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
    await saveServer(next);
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

  /** Pair with a scanned/pasted payload. Resolves with the stored server. */
  async pair(payload: PairingPayload): Promise<StoredServer> {
    const client = new GatewayClient({
      hosts: payload.hosts.map((h) => parseHostEntry(h, payload.port)),
      sid: payload.sid,
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

  /** Forget this phone on the server (if reachable) and locally. */
  async remove(serverId: string): Promise<void> {
    const c = this.clients.get(serverId);
    if (c?.ready) {
      try {
        await c.forget();
      } catch {
        // remove locally anyway
      }
    }
    c?.close();
    this.clients.delete(serverId);
    await deleteServer(serverId);
    this.servers = this.servers.filter((s) => s.server_id !== serverId);
    this.changed();
  }
}

export const connections = new Connections();

export type { ConnStatus };
