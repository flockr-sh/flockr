import { WebSocket } from 'ws';
import { MockGateway } from '../../mock/gateway';
import {
  AgentsMessage,
  buildAgentRows,
  ClientOptions,
  Credentials,
  decodeServerKey,
  FatalInfo,
  GatewayClient,
  GatewayError,
  gridText,
  NotifyMessage,
  parseHostEntry,
  parsePairingUrl,
  Subscription,
  WebSocketLike,
} from '../../src/protocol';
import { generateKeyPair, HANDSHAKE_MSG_LEN } from '../../src/protocol/noise';

function keyOf(pk: string): Uint8Array {
  const k = decodeServerKey(pk);
  if (!k) throw new Error('bad pk');
  return k;
}

const createSocket = (url: string, protocols: string[]) => new WebSocket(url, protocols) as unknown as WebSocketLike;

function waitFor(cond: () => boolean, timeoutMs = 3000): Promise<void> {
  return new Promise((resolve, reject) => {
    const start = Date.now();
    const tick = () => {
      if (cond()) return resolve();
      if (Date.now() - start > timeoutMs) return reject(new Error('waitFor timed out'));
      setTimeout(tick, 5);
    };
    tick();
  });
}

describe('GatewayClient against the mock gateway', () => {
  let gw: MockGateway;
  const clients: GatewayClient[] = [];

  beforeEach(async () => {
    gw = new MockGateway({ animate: false });
    await gw.start();
  });

  afterEach(async () => {
    for (const c of clients.splice(0)) c.close();
    await gw.stop();
  });

  function makeClient(credentials: Credentials, extra: Partial<ClientOptions> = {}, url?: string): GatewayClient {
    const p = parsePairingUrl(url ?? gw.createPairing());
    const c = new GatewayClient({
      hosts: p.hosts.map((h) => parseHostEntry(h, p.port)),
      sid: p.sid,
      serverKey: keyOf(p.pk),
      credentials,
      app: { version: 'test', platform: 'node' },
      createSocket,
      pingIntervalMs: 0,
      reconnect: false,
      ...extra,
    });
    clients.push(c);
    return c;
  }

  async function pairedClient(
    extra: Partial<ClientOptions> = {},
  ): Promise<{ client: GatewayClient; token: string; device_id: string }> {
    const url = gw.createPairing();
    const p = parsePairingUrl(url);
    const client = makeClient({ kind: 'pair', code: p.code, device_name: 'test phone' }, extra, url);
    let token = '';
    let device_id = '';
    client.on('paired', (d) => {
      token = d.token;
      device_id = d.device_id;
    });
    client.connect();
    await client.waitReady();
    return { client, token, device_id };
  }

  it('pairs, then reconnects with the device token', async () => {
    const { client, token, device_id } = await pairedClient();
    expect(token).toMatch(/^fmd1_[A-Za-z0-9_-]{43}$/);
    expect(device_id).toMatch(/^d_/);
    expect(client.hasFeature('surface')).toBe(true);
    client.close();

    const again = makeClient({ kind: 'auth', device_id, token });
    again.connect();
    const w = await again.waitReady();
    expect(w.device.token).toBeUndefined();
    expect(w.server.server_id).toBe(gw.sid);
    expect(gw.received.filter((m) => m.t === 'hello').map((m) => Object.keys(m).includes('auth'))).toEqual([
      false,
      true,
    ]);
  });

  it('a used pairing code fails as terminal (4005)', async () => {
    const url = gw.createPairing();
    const p = parsePairingUrl(url);
    const first = makeClient({ kind: 'pair', code: p.code, device_name: 'a' }, {}, url);
    first.connect();
    await first.waitReady();
    const second = makeClient({ kind: 'pair', code: p.code, device_name: 'b' }, { reconnect: true }, url);
    const fatal = new Promise<FatalInfo>((r) => second.on('fatal', r));
    second.connect();
    const f = await fatal;
    expect(f).toMatchObject({ code: 4005, error: 'pairing_failed', tokenDead: false });
  });

  it('a bad token is terminal and marks the token dead (4001)', async () => {
    const { device_id } = await pairedClient();
    const c = makeClient({ kind: 'auth', device_id, token: 'fmd1_wrong' }, { reconnect: true });
    const fatal = new Promise<FatalInfo>((r) => c.on('fatal', r));
    c.connect();
    expect(await fatal).toMatchObject({ code: 4001, error: 'unauthorized', tokenDead: true });
  });

  it('rejects a server whose id does not match the pairing sid', async () => {
    const { token, device_id } = await pairedClient();
    const p = parsePairingUrl(gw.createPairing());
    const c = new GatewayClient({
      hosts: [{ host: '127.0.0.1', port: p.port }],
      sid: 'not-this-server',
      serverKey: keyOf(p.pk),
      credentials: { kind: 'auth', device_id, token },
      app: { version: 'test', platform: 'node' },
      createSocket,
      pingIntervalMs: 0,
    });
    clients.push(c);
    const fatal = new Promise<FatalInfo>((r) => c.on('fatal', r));
    c.connect();
    expect((await fatal).error).toBe('server_mismatch');
  });

  it('a different server key fails the handshake (4006): terminal, token kept', async () => {
    const { token, device_id } = await pairedClient();
    const c = makeClient(
      { kind: 'auth', device_id, token },
      { reconnect: true, serverKey: generateKeyPair().publicKey },
    );
    const fatal = new Promise<FatalInfo>((r) => c.on('fatal', r));
    c.connect();
    expect(await fatal).toMatchObject({ code: 4006, error: 'handshake_failed', tokenDead: false });
    expect(c.connStatus).toMatchObject({ state: 'failed', error: 'handshake_failed' });
    expect(gw.received.filter((m) => m.t === 'hello')).toHaveLength(1);
  });

  it('hello carries versions [2]', async () => {
    await pairedClient();
    const hello = gw.received.find((m) => m.t === 'hello');
    expect(hello?.versions).toEqual([2]);
  });

  it('a text frame from the server is a protocol error', async () => {
    const { client } = await pairedClient({ reconnect: true, random: () => 0 });
    const statuses: string[] = [];
    client.on('status', (s) => statuses.push(`${s.state}:${s.code ?? ''}`));
    for (const ws of (gw as unknown as { wss: { clients: Set<WebSocket> } }).wss.clients) ws.send('{"t":"pong"}');
    await waitFor(() => statuses.includes('waiting:4000'));
  });

  it('revocation closes with 4003 and kills the token', async () => {
    const { client, device_id } = await pairedClient({ reconnect: true });
    const fatal = new Promise<FatalInfo>((r) => client.on('fatal', r));
    gw.revoke(device_id);
    expect(await fatal).toMatchObject({ code: 4003, tokenDead: true });
  });

  it('falls through to the next host when the first is unreachable', async () => {
    const url = gw.createPairing(600, ['127.0.0.1:1', '127.0.0.1']);
    const p = parsePairingUrl(url);
    const c = makeClient({ kind: 'pair', code: p.code, device_name: 'x' }, { reconnect: true }, url);
    c.connect();
    await c.waitReady();
    expect(c.currentHostIndex).toBe(1);
  });

  it('a one-shot connect (reconnect:false) still tries every host', async () => {
    const url = gw.createPairing(600, ['127.0.0.1:1', '127.0.0.1']);
    const p = parsePairingUrl(url);
    const c = makeClient({ kind: 'pair', code: p.code, device_name: 'x' }, { reconnect: false }, url);
    c.connect();
    await c.waitReady();
    expect(c.currentHostIndex).toBe(1);
  });

  it('streams the agent list and sorts it', async () => {
    const { client } = await pairedClient();
    const got: AgentsMessage[] = [];
    client.watchAgents((m) => got.push(m));
    await waitFor(() => got.length === 1);
    expect(buildAgentRows(got[0]).map((r) => r.status)).toEqual(['blocked', 'working', 'working', 'idle']);
    gw.setStatus('p_3', 'blocked');
    await waitFor(() => got.length === 2);
    expect(got[1].rev).toBe(2);
    expect(buildAgentRows(got[1])[1].agent.pane_id).toBe('p_3');
  });

  it('sends in-band notify only while foreground', async () => {
    const { client } = await pairedClient();
    const notes: NotifyMessage[] = [];
    client.on('notify', (n) => notes.push(n));
    gw.setStatus('p_2', 'blocked');
    client.setForeground(true);
    await waitFor(() => gw.received.some((m) => m.t === 'app.state'));
    gw.setStatus('p_2', 'done');
    await waitFor(() => notes.length === 1);
    expect(notes[0]).toMatchObject({ kind: 'done', pane_id: 'p_2', title: 'Codex finished' });
  });

  it('API passthrough returns the API line; disallowed methods error', async () => {
    const { client } = await pairedClient();
    const line = await client.request('agent.list');
    expect(line.result?.type).toBe('agent_list');
    const r = await client.api<{ workspaces: unknown[] }>('workspace.list');
    expect(r.workspaces).toHaveLength(2);
    await expect(client.request('server.stop')).rejects.toMatchObject({ code: 'method_not_allowed' });
  });

  async function subscribed(client: GatewayClient, target: string, opts = {}): Promise<Subscription> {
    const sub = client.subscribe(target, opts);
    await waitFor(() => sub.grid.seq > 0);
    return sub;
  }

  it('subscribes, gets a frame, and input echoes as a delta', async () => {
    const { client } = await pairedClient();
    const sub = await subscribed(client, 'p_3');
    expect(sub.started?.pane_id).toBe('p_3');
    expect(sub.grid.cols).toBe(100);
    expect(gridText(sub.grid)[1]).toBe('$');
    const rtt = await sub.input('text', 'ls -la');
    expect(rtt).toBeGreaterThanOrEqual(0);
    await waitFor(() => gridText(sub.grid)[1] === '$ ls -la');
    expect(sub.echo.last).not.toBeNull();
    await sub.input('key', ['enter']);
    await waitFor(() => gridText(sub.grid)[2] === 'mock: ran "ls -la"');
    await sub.input('submit', 'pwd');
    await waitFor(() => gridText(sub.grid).join('\n').includes('mock: ran "pwd"'));
    await expect(sub.input('key', ['hyper+q'])).rejects.toBeInstanceOf(GatewayError);
    const acks = gw.received.filter((m) => m.t === 'surface.ack');
    expect(acks.length).toBeGreaterThan(0);
    expect(sub.stats.snapshot().updatesPerSec).toBeGreaterThan(0);
  });

  it('respects max_in_flight when the app holds acks', async () => {
    const { client } = await pairedClient({ maxInFlight: 2 });
    const sub = await subscribed(client, 'p_3', { autoAck: false });
    const pane = gw.pane('p_3');
    if (!pane) throw new Error('no pane');
    for (let i = 0; i < 5; i++) {
      pane.term.writeLine(`line ${i}`);
      gw.pumpAll();
      await new Promise((r) => setTimeout(r, 40));
    }
    // Frame (seq 1) + one delta, then the server waits for acks.
    expect(sub.grid.seq).toBe(2);
    sub.ack();
    await waitFor(() => sub.grid.seq === 3);
    sub.ack();
    await waitFor(() => gridText(sub.grid).join('\n').includes('line 4'));
  });

  it('resyncs on a base mismatch', async () => {
    const { client } = await pairedClient();
    const sub = await subscribed(client, 'p_3');
    sub.grid.seq = 999; // corrupt local state
    gw.pane('p_3')?.term.writeLine('next');
    gw.pumpAll();
    await waitFor(() => sub.stats.resyncs === 1);
    await waitFor(() => gw.received.some((m) => m.t === 'surface.resync'));
    await waitFor(() => gridText(sub.grid)[1] === '$ next' && sub.grid.seq === 3);
  });

  it('fit mode resizes the pane and restores it on unsubscribe', async () => {
    const { client } = await pairedClient();
    const sub = await subscribed(client, 'p_1', { mode: 'fit', cols: 46, rows: 30 });
    expect(sub.grid.cols).toBe(46);
    expect(gw.pane('p_1')?.term.cols).toBe(46);
    sub.resize(50, 20);
    await waitFor(() => sub.grid.cols === 50 && sub.grid.rows === 20);
    const busy = client.subscribe('p_1', { mode: 'fit', cols: 10, rows: 10 });
    const ended = new Promise<string>((r) => (busy.handlers.onEnded = (reason, msg) => r(`${reason}:${msg}`)));
    expect(await ended).toMatch(/pane_busy/);
    sub.unsubscribe();
    await waitFor(() => gw.pane('p_1')?.term.cols === 100);
  });

  it('history returns plain lines', async () => {
    const { client } = await pairedClient();
    const sub = await subscribed(client, 'p_3');
    const lines = await sub.history(100);
    expect(lines.some((l) => l.includes('mock flockr shell'))).toBe(true);
  });

  it('push register validates the token', async () => {
    const { client } = await pairedClient();
    await client.registerPush('ExponentPushToken[abc_DEF-123]', 'ios');
    expect(gw.devices[0].expo_token).toBe('ExponentPushToken[abc_DEF-123]');
    await expect(client.registerPush('nope', 'ios')).rejects.toMatchObject({ code: 'bad_request' });
  });

  it('ping measures RTT', async () => {
    const { client } = await pairedClient();
    client.ping();
    await waitFor(() => client.rtt.last !== null);
    expect(client.rtt.last).toBeGreaterThanOrEqual(0);
  });

  it('reconnects after the server drops and resubscribes', async () => {
    const { client, device_id, token } = await pairedClient({ reconnect: true, random: () => 0 });
    expect(device_id && token).toBeTruthy();
    const sub = await subscribed(client, 'p_3');
    const welcomes: number[] = [];
    client.on('welcome', () => welcomes.push(1));
    // Drop all connections server-side.
    for (const ws of (gw as unknown as { wss: { clients: Set<WebSocket> } }).wss.clients) ws.terminate();
    await waitFor(() => welcomes.length === 1, 5000);
    await waitFor(() => sub.grid.seq === 1 && client.ready);
    expect(gridText(sub.grid)[1]).toBe('$');
  });

  it('resubscribes a surface that ended with server_shutdown', async () => {
    const { client } = await pairedClient({ reconnect: true, random: () => 0 });
    const sub = await subscribed(client, 'p_3');
    const welcomes: number[] = [];
    client.on('welcome', () => welcomes.push(1));
    // What a server stop or live handoff sends: surface.ended, then a close.
    gw.sendToAll({ t: 'surface.ended', sub: sub.sub, reason: 'server_shutdown' }, 4009);
    await waitFor(() => welcomes.length === 1, 5000);
    await waitFor(() => sub.grid.seq === 1 && client.ready);
    expect(sub.ended).toBeNull();
    expect(gridText(sub.grid)[1]).toBe('$');
  });

  it('device.forget removes the device', async () => {
    const { client } = await pairedClient();
    await client.forget();
    expect(gw.devices).toHaveLength(0);
  });
});

/** A socket that answers message 1 with `reply` and records what the client sent. */
function fakeSocket(reply: (msg1: Uint8Array) => unknown) {
  const sent: Uint8Array[] = [];
  const closes: number[] = [];
  const sock: WebSocketLike = {
    readyState: 0,
    send(data: Uint8Array) {
      sent.push(data);
      if (sent.length === 1) setTimeout(() => sock.onmessage?.({ data: reply(data) }), 0);
    },
    close(code?: number) {
      closes.push(code ?? 1000);
    },
    onopen: null,
    onmessage: null,
    onclose: null,
    onerror: null,
  };
  setTimeout(() => {
    (sock as { readyState: number }).readyState = 1;
    sock.onopen?.({});
  }, 0);
  return { sock, sent, closes };
}

describe('GatewayClient handshake failures (fake socket)', () => {
  const base = {
    hosts: [{ host: '127.0.0.1', port: 1 }],
    sid: 's',
    credentials: { kind: 'auth', device_id: 'd', token: 't' } as Credentials,
    app: { version: 'test', platform: 'node' },
    pingIntervalMs: 0,
    reconnect: true,
  };

  it('message 2 that does not decrypt is terminal handshake_failed', async () => {
    const f = fakeSocket(() => new Uint8Array(HANDSHAKE_MSG_LEN).fill(9).buffer);
    const c = new GatewayClient({ ...base, serverKey: generateKeyPair().publicKey, createSocket: () => f.sock });
    const fatal = new Promise<FatalInfo>((r) => c.on('fatal', r));
    c.connect();
    expect(await fatal).toMatchObject({ code: 4006, error: 'handshake_failed', tokenDead: false });
    expect(f.sent).toHaveLength(1);
    expect(f.sent[0]).toHaveLength(HANDSHAKE_MSG_LEN);
    expect(f.closes).toEqual([4000]);
    c.close();
  });

  it('message 2 of the wrong length is terminal handshake_failed', async () => {
    const f = fakeSocket(() => new Uint8Array(47).buffer);
    const c = new GatewayClient({ ...base, serverKey: generateKeyPair().publicKey, createSocket: () => f.sock });
    const fatal = new Promise<FatalInfo>((r) => c.on('fatal', r));
    c.connect();
    expect((await fatal).error).toBe('handshake_failed');
    c.close();
  });

  it('a text frame instead of message 2 is a protocol error, not a handshake failure', async () => {
    const f = fakeSocket(() => '{"t":"welcome"}');
    const c = new GatewayClient({ ...base, serverKey: generateKeyPair().publicKey, createSocket: () => f.sock });
    const status = new Promise<string>((r) =>
      c.on('status', (s) => {
        if (s.state === 'waiting' || s.state === 'failed') r(`${s.state}:${s.code}`);
      }),
    );
    c.connect();
    expect(await status).toBe('waiting:4000');
    c.close();
  });
});
