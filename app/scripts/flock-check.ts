// Checks phone sessions relayed through a flock: pairs with one server, then
// opens a session to every other machine in its flock through it.
//
//   npx tsx scripts/flock-check.ts --pair 'flockr://pair?p=...'
//
// Get the URL with `flockr mobile pair --no-wait --json` on the server.
import { WebSocket } from 'ws';
import {
  AgentsMessage,
  buildAgentRows,
  Credentials,
  decodeServerKey,
  GatewayClient,
  gridText,
  parseHostEntry,
  parsePairingUrl,
  PairingPayload,
  WebSocketFactory,
  WebSocketLike,
} from '../src/protocol';

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const createSocket: WebSocketFactory = (url, protocols) => {
  const ws = new WebSocket(url, protocols);
  const like = {
    get readyState() {
      return ws.readyState;
    },
    set binaryType(t: string) {
      ws.binaryType = t as 'arraybuffer';
    },
    get binaryType() {
      return ws.binaryType;
    },
    send: (d: Uint8Array) => ws.send(d),
    close: (code?: number, reason?: string) => ws.close(code, reason),
    onopen: null as WebSocketLike['onopen'],
    onmessage: null as WebSocketLike['onmessage'],
    onclose: null as WebSocketLike['onclose'],
    onerror: null as WebSocketLike['onerror'],
  };
  ws.on('open', () => like.onopen?.({}));
  ws.on('message', (data, isBinary) => like.onmessage?.({ data: isBinary ? data : data.toString() }));
  ws.on('close', (code, reason) => like.onclose?.({ code, reason: reason.toString() }));
  ws.on('error', (e) => like.onerror?.(e));
  return like;
};

let failures = 0;
function check(name: string, ok: boolean, detail?: unknown): boolean {
  if (!ok) failures++;
  console.log(`${ok ? 'PASS' : 'FAIL'} ${name}${ok || detail === undefined ? '' : ` -- ${JSON.stringify(detail)}`}`);
  return ok;
}

async function waitFor<T>(what: string, fn: () => T | undefined | null | false, ms = 5000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = fn();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timeout waiting for ${what}`);
    await sleep(25);
  }
}

function newClient(payload: PairingPayload, credentials: Credentials, machine?: string): GatewayClient {
  const serverKey = decodeServerKey(payload.pk);
  if (!serverKey) throw new Error('pairing payload has no valid pk');
  return new GatewayClient({
    hosts: payload.hosts.map((h) => parseHostEntry(h, payload.port)),
    sid: payload.sid,
    machine,
    serverKey,
    credentials,
    app: { version: '0.1.0', platform: 'node' },
    createSocket,
    reconnect: false,
    pingIntervalMs: 0,
  });
}

async function main(): Promise<void> {
  const url = arg('--pair');
  if (!url) throw new Error("need --pair 'flockr://pair?p=...'");
  const payload = parsePairingUrl(url, Math.floor(Date.now() / 1000));

  const pairClient = newClient(payload, { kind: 'pair', code: payload.code, device_name: 'flock-check' });
  let paired: { device_id: string; token: string } | null = null;
  pairClient.on('paired', (d) => (paired = d));
  pairClient.connect();
  const welcome = await pairClient.waitReady();
  const creds = await waitFor('paired event', () => paired);
  pairClient.close();
  const machines = welcome.machines ?? [];
  console.log(
    `  ${welcome.server.name}: ${machines.map((m) => `${m.name}${m.this ? ' (this)' : ''}${m.online ? '' : ' offline'}`).join(', ')}`,
  );
  check('welcome lists the flock', machines.some((m) => m.this) && machines.length > 1, welcome);

  const auth: Credentials = { kind: 'auth', device_id: creds.device_id, token: creds.token };
  for (const machine of machines.filter((m) => !m.this)) {
    const client = newClient(payload, auth, machine.id);
    let fatal: unknown = null;
    client.on('fatal', (f) => (fatal = f));
    client.connect();
    let w;
    try {
      w = await client.waitReady(10000);
    } catch (e) {
      check(`${machine.name}: relayed welcome`, false, { error: String(e), fatal });
      client.close();
      continue;
    }
    check(
      `${machine.name}: relayed welcome`,
      w.machine === machine.id && w.server.server_id === payload.sid && w.server.name === machine.name,
      { machine: w.machine, server: w.server },
    );

    let agentsMsg: AgentsMessage | null = null;
    const unwatch = client.watchAgents((m) => (agentsMsg = m));
    await waitFor('agents', () => agentsMsg, 10000);
    unwatch();
    const rows = buildAgentRows(agentsMsg as unknown as AgentsMessage);
    for (const r of rows) console.log(`  ${machine.name}: ${r.status.padEnd(8)} ${r.name} (${r.agent.pane_id})`);
    check(`${machine.name}: agents`, rows.length > 0, agentsMsg);

    const panes = await client.request('pane.list');
    const pane = arg('--target') ?? (panes.result as { panes: { pane_id: string }[] }).panes[0]?.pane_id;
    if (!pane) {
      check(`${machine.name}: a pane to open`, false, panes);
      client.close();
      continue;
    }
    const sub = client.subscribe(pane, { mode: 'fit', cols: 60, rows: 20 }, {});
    await waitFor('first frame', () => sub.grid.seq > 0 || sub.ended, 10000);
    check(`${machine.name}: surface frame for ${pane}`, sub.grid.seq > 0, sub.ended);
    const marker = `relayed${Math.floor(Math.random() * 1e6)}`;
    await sub.input('text', `echo ${marker}`);
    await sub.input('submit', '');
    const echoed = await waitFor(
      'command output',
      () => gridText(sub.grid).some((l) => l.trim() === marker),
      10000,
    ).catch(() => false);
    check(`${machine.name}: typed command ran`, !!echoed, gridText(sub.grid).join('\n'));
    sub.unsubscribe();
    client.close();
  }

  console.log(failures === 0 ? 'all checks passed' : `${failures} check(s) failed`);
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
