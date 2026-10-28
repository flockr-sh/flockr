// End-to-end smoke test for the protocol client under Node.
//
//   npm run smoke -- --flockr <path/to/flockr>   # real gateway; the flockr CLI must reach the server
//   npm run smoke -- --mock                      # in-process mock gateway
//   npm run smoke -- --pair 'flockr://pair?p=...' --target w1:p1   # pair by hand, no revoke step
//
// With --flockr the script runs `flockr mobile pair --no-wait --json` to get a code,
// `flockr pane report-agent` to fake a blocked agent, and `flockr mobile devices revoke`
// at the end. Run it with the same env (XDG_*, FLOCKR_SOCKET_PATH unset) as the server.
//
// Options: --target <pane> (default: first agent), --busy-seconds <n> (default 3),
//          --measure-only (skip revoke)
import { execFileSync } from 'node:child_process';
import { WebSocket } from 'ws';
import { MockGateway } from '../mock/gateway';
import {
  AgentsMessage,
  buildAgentRows,
  Credentials,
  decodeServerKey,
  GatewayClient,
  GatewayError,
  Grid,
  gridText,
  NotifyMessage,
  parseHostEntry,
  parsePairingUrl,
  PairingPayload,
  Subscription,
  SurfaceHandlers,
  SubscribeOptions,
  WebSocketFactory,
  WebSocketLike,
} from '../src/protocol';

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// --- byte accounting per message type ---

interface Sizes {
  count: number;
  bytes: number;
  max: number;
  list: number[];
}
const sizes = new Map<string, Sizes>();
function record(data: string): void {
  const m = /^\{"t":"([^"]+)"/.exec(data);
  const t = m ? m[1] : '?';
  let s = sizes.get(t);
  if (!s) sizes.set(t, (s = { count: 0, bytes: 0, max: 0, list: [] }));
  const n = Buffer.byteLength(data);
  s.count++;
  s.bytes += n;
  s.max = Math.max(s.max, n);
  s.list.push(n);
}
function resetSizes(): void {
  sizes.clear();
}
function sizeSummary(t: string): string {
  const s = sizes.get(t);
  if (!s || s.count === 0) return `${t}: none`;
  const sorted = [...s.list].sort((a, b) => a - b);
  const med = sorted[Math.floor(sorted.length / 2)];
  return `${t}: n=${s.count} avg=${Math.round(s.bytes / s.count)}B median=${med}B max=${s.max}B total=${(s.bytes / 1024).toFixed(1)}KB`;
}

// Plain `ws` socket; the client sets binaryType to 'arraybuffer'. Message
// sizes are counted from the decrypted JSON through `onMessageText`.
let binaryFrames = 0;
let textFrames = 0;
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
  ws.on('message', (data, isBinary) => {
    if (isBinary) binaryFrames++;
    else textFrames++;
    like.onmessage?.({ data: isBinary ? data : data.toString() });
  });
  ws.on('close', (code, reason) => like.onclose?.({ code, reason: reason.toString() }));
  ws.on('error', (e) => like.onerror?.(e));
  return like;
};

// --- checks ---

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

// --- targets: the real flockr server through its CLI, or the mock ---

interface Target {
  kind: 'flockr' | 'mock' | 'manual';
  pairUrl(): Promise<string>;
  setStatus?(pane: string, status: 'working' | 'blocked' | 'idle'): Promise<void>;
  /** Undo setStatus so the pane is left as it was. */
  clearStatus?(pane: string): Promise<void>;
  /** Make the pane's agent finish work, so it shows as done until read. */
  finish?(pane: string): Promise<void>;
  revoke?(deviceId: string): Promise<void>;
  devices?(): Promise<string[]>;
  stop(): Promise<void>;
}

function flockrTarget(bin: string): Target {
  const run = (...args: string[]) => execFileSync(bin, args, { encoding: 'utf8' });
  return {
    kind: 'flockr',
    async pairUrl() {
      const out = JSON.parse(run('mobile', 'pair', '--no-wait', '--json')) as { url: string };
      return out.url;
    },
    async setStatus(pane, status) {
      run('pane', 'report-agent', '--source', 'smoke', '--agent', 'claude', '--state', status, pane);
    },
    async clearStatus(pane) {
      run('pane', 'release-agent', '--source', 'smoke', '--agent', 'claude', pane);
    },
    async finish(pane) {
      run('pane', 'report-agent', '--source', 'smoke', '--agent', 'claude', '--state', 'working', pane);
      run('pane', 'report-agent', '--source', 'smoke', '--agent', 'claude', '--state', 'idle', pane);
    },
    async revoke(deviceId) {
      run('mobile', 'devices', 'revoke', deviceId);
    },
    async devices() {
      const out = JSON.parse(run('mobile', 'devices', 'list', '--json')) as unknown;
      const list = Array.isArray(out) ? out : ((out as { devices?: unknown[] }).devices ?? []);
      return list.map((d) => String((d as { device_id: string }).device_id));
    },
    async stop() {},
  };
}

async function mockTarget(): Promise<{ target: Target; mock: MockGateway }> {
  const mock = new MockGateway({ log: () => {} });
  await mock.start();
  return {
    mock,
    target: {
      kind: 'mock',
      async pairUrl() {
        return mock.createPairing();
      },
      async setStatus(pane, status) {
        mock.setStatus(pane, status);
      },
      async clearStatus(pane) {
        mock.setStatus(pane, 'working');
      },
      async finish(pane) {
        mock.setStatus(pane, 'working');
        mock.setStatus(pane, 'done');
      },
      async revoke(deviceId) {
        mock.revoke(deviceId);
      },
      async stop() {
        await mock.stop();
      },
    },
  };
}

function newClient(payload: PairingPayload, credentials: Credentials): GatewayClient {
  const serverKey = decodeServerKey(payload.pk);
  if (!serverKey) throw new Error('pairing payload has no valid pk');
  return new GatewayClient({
    hosts: payload.hosts.map((h) => parseHostEntry(h, payload.port)),
    sid: payload.sid,
    serverKey,
    onMessageText: record,
    credentials,
    app: { version: '0.1.0', platform: 'node' },
    createSocket,
    reconnect: false,
    pingIntervalMs: 0,
  });
}

function screen(grid: Grid): string {
  return gridText(grid).join('\n');
}

async function openSub(
  client: GatewayClient,
  target: string,
  opts: SubscribeOptions,
  handlers: SurfaceHandlers = {},
): Promise<Subscription> {
  const sub = client.subscribe(target, opts, handlers);
  await waitFor('surface.started', () => sub.started || sub.ended, 5000);
  if (sub.ended) throw new Error(`subscription ended: ${sub.ended}`);
  await waitFor('first frame', () => sub.grid.seq > 0, 5000);
  return sub;
}

async function main(): Promise<void> {
  let target: Target;
  const flockrBin = arg('--flockr');
  if (flockrBin) target = flockrTarget(flockrBin);
  else if (process.argv.includes('--mock')) target = (await mockTarget()).target;
  else {
    const url = arg('--pair');
    if (!url) throw new Error('need --flockr <bin>, --mock or --pair <url>');
    target = { kind: 'manual', pairUrl: async () => url, stop: async () => {} };
  }
  const isReal = target.kind !== 'mock';
  const busySeconds = Number(arg('--busy-seconds') ?? 3);

  // 1. Pair.
  const pairUrl = await target.pairUrl();
  const payload = parsePairingUrl(pairUrl, Math.floor(Date.now() / 1000));
  check('pairing url parses', payload.hosts.length > 0 && !!payload.sid, payload);
  const pairClient = newClient(payload, { kind: 'pair', code: payload.code, device_name: 'smoke-test' });
  let paired: { device_id: string; token: string } | null = null;
  pairClient.on('paired', (d) => (paired = d));
  pairClient.connect();
  const welcome = await pairClient.waitReady();
  check('pair: welcome', welcome.version === 2 && welcome.server.server_id === payload.sid, welcome.server);
  await waitFor('paired event', () => paired);
  const creds = paired as unknown as { device_id: string; token: string };
  check('pair: device token issued', creds.token.startsWith('fmd1_') || !isReal, creds.device_id);
  pairClient.close();

  // 2. Reusing the code fails.
  {
    const again = newClient(payload, { kind: 'pair', code: payload.code, device_name: 'smoke-again' });
    again.connect();
    let err = '';
    try {
      await again.waitReady(5000);
    } catch (e) {
      err = (e as GatewayError).code;
    }
    check('pair: reused code refused', err === 'pairing_failed', err);
    again.close();
  }

  // 3. Connect with the device token.
  const client = newClient(payload, { kind: 'auth', device_id: creds.device_id, token: creds.token });
  const notifies: NotifyMessage[] = [];
  client.on('notify', (n) => notifies.push(n));
  let fatal: { code: number; error: string } | null = null;
  client.on('fatal', (f) => (fatal = f));
  client.connect();
  const w2 = await client.waitReady();
  check('auth: welcome', w2.device.device_id === creds.device_id, w2.device);
  console.log(`  features: ${w2.features.join(', ')}`);

  // 4. API passthrough, and a blocked method.
  const ping = await client.request('ping');
  check('api: ping', !ping.error, ping);
  if (isReal) {
    const blocked = await client.request('server.stop').catch((e: GatewayError) => ({ error: { code: e.code } }));
    check('api: server.stop refused', blocked.error?.code === 'method_not_allowed', blocked);
  }

  // 5. Agents.
  let agentsMsg: AgentsMessage | null = null;
  const unwatch = client.watchAgents((m) => (agentsMsg = m));
  await waitFor('agents', () => agentsMsg);
  const rows = buildAgentRows(agentsMsg as unknown as AgentsMessage);
  for (const r of rows)
    console.log(
      `  ${r.status.padEnd(8)} ${r.name.padEnd(10)} ${r.workspaceLabel} > ${r.tabLabel} (${r.agent.pane_id})` +
        `${r.branch ? ` branch=${r.branch}` : ''}${r.sessionEmpty ? ' session_empty' : ''}`,
    );
  check('agents: list not empty', rows.length > 0, agentsMsg);
  if (target.kind === 'mock') {
    check(
      'agents: branch and session_empty pass through',
      rows.some((r) => r.branch === 'mobile-agents') && rows.some((r) => r.sessionEmpty),
      rows.map((r) => [r.agent.pane_id, r.branch, r.sessionEmpty]),
    );
  }
  // A plain shell pane: the faked `claude` agent on flockr, the `shell` pane on the mock.
  const want = isReal ? 'claude' : 'shell';
  const pane = arg('--target') ?? rows.find((r) => r.name === want)?.agent.pane_id ?? rows[0]?.agent.pane_id;
  if (!pane) throw new Error('no pane to subscribe to');
  console.log(`  target pane: ${pane}`);

  // 6. Observe subscription: full frame.
  resetSizes();
  const obs = await openSub(client, pane, { mode: 'observe' });
  check('surface: observe started', obs.started?.pane_id === pane, obs.started);
  check('surface: full frame', obs.grid.cols > 0 && obs.grid.rows > 0, { cols: obs.grid.cols, rows: obs.grid.rows });
  console.log(`  observe size ${obs.grid.cols}x${obs.grid.rows}; ${sizeSummary('surface.frame')}`);
  obs.unsubscribe();

  // 7. Fit mode at an iPhone-sized grid; type a command and see it echoed.
  resetSizes();
  const fit = await openSub(client, pane, { mode: 'fit', cols: 45, rows: 30 });
  check('surface: fit 45x30', fit.grid.cols === 45 && fit.grid.rows === 30, {
    cols: fit.grid.cols,
    rows: fit.grid.rows,
  });
  const fitFrame = sizes.get('surface.frame');
  const marker = `smoke${Math.floor(Math.random() * 1e6)}`;
  if (isReal) {
    await fit.input('key', ['ctrl+c']);
    await fit.input('text', 'clear');
    await fit.input('key', 'enter');
    await sleep(500);
  }
  resetSizes();
  const ms = await fit.input('text', `echo ${marker}`);
  check('input: text ok', ms >= 0);
  await waitFor('typed text echoed', () => screen(fit.grid).includes(`echo ${marker}`), 5000).then(
    () => check('input: typed text echoed in delta', (sizes.get('surface.delta')?.count ?? 0) > 0),
    (e) => check('input: typed text echoed in delta', false, String(e)),
  );
  const typeDeltas = sizes.get('surface.delta');
  console.log(`  typing echo: ${sizeSummary('surface.delta')} echo=${fit.echo.last?.toFixed(1) ?? '-'}ms`);
  await fit.input('submit', '');
  await waitFor(
    'command output',
    () =>
      screen(fit.grid)
        .split('\n')
        .some((l) => l.trim() === marker || (!isReal && l.includes(`ran "echo ${marker}"`))),
    5000,
  ).then(
    () => check('input: submit runs command', true),
    (e) => check('input: submit runs command', false, String(e) + '\n' + screen(fit.grid)),
  );
  // Every key-row name the app's Composer sends, except ctrl+d and enter
  // (both would act on the shell) and the plain characters.
  for (const k of ['esc', 'tab', 'shift+tab', 'left', 'up', 'down', 'right', 'backspace', 'ctrl+c']) {
    await fit.input('key', [k]).then(
      () => check(`input: key ${k}`, true),
      (e) => check(`input: key ${k}`, false, String(e)),
    );
  }
  // An attached image comes back as a pasted file path. 1x1 PNG.
  if (client.hasFeature('input.image')) {
    const png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==';
    await fit.image('png', png).then(
      () => check('input: image ok', true),
      (e) => check('input: image ok', false, String(e)),
    );
    await waitFor('image path pasted', () => /\S+\.png/.test(screen(fit.grid)), 5000).then(
      () => check('input: image path pasted', true),
      (e) => check('input: image path pasted', false, String(e) + '\n' + screen(fit.grid)),
    );
    await fit.input('key', ['ctrl+c']);
  }

  // 8. One-character delta (typical keystroke).
  await sleep(300);
  resetSizes();
  await fit.input('text', 'x');
  await waitFor('one char delta', () => sizes.get('surface.delta'), 3000).catch(() => null);
  await sleep(200);
  const oneChar = sizeSummary('surface.delta');
  console.log(`  one keystroke: ${oneChar}`);
  await fit.input('key', 'backspace');

  // 9. Resize the fit subscription.
  resetSizes();
  fit.resize(40, 20);
  await waitFor('resized frame', () => fit.grid.cols === 40 && fit.grid.rows === 20, 5000).then(
    () => check('surface: resize to 40x20', true),
    () => check('surface: resize to 40x20', false, { cols: fit.grid.cols, rows: fit.grid.rows }),
  );
  fit.resize(45, 30);
  await waitFor('resized back', () => fit.grid.cols === 45 && fit.grid.rows === 30, 5000).catch(() => null);

  // 10. Busy output: measure deltas at 45x30.
  let busy = '';
  if (isReal) {
    await sleep(300);
    resetSizes();
    const t0 = Date.now();
    await fit.input(
      'text',
      `end=$((SECONDS+${busySeconds})); i=0; while [ $SECONDS -lt $end ]; do printf 'line %d the quick brown fox jumps over the lazy dog\\n' $i; i=$((i+1)); done`,
    );
    await fit.input('submit', '');
    await sleep(busySeconds * 1000 + 800);
    const secs = (Date.now() - t0) / 1000;
    const d = sizes.get('surface.delta');
    const f = sizes.get('surface.frame');
    const total = (d?.bytes ?? 0) + (f?.bytes ?? 0);
    busy = `busy ${secs.toFixed(1)}s: ${sizeSummary('surface.delta')}; ${sizeSummary('surface.frame')}; ${(((d?.count ?? 0) + (f?.count ?? 0)) / secs) | 0} updates/s, ${(total / 1024 / secs).toFixed(1)} KB/s`;
    console.log(`  ${busy}`);
    check('busy: updates flowed', (d?.count ?? 0) + (f?.count ?? 0) > 5, busy);
    check('busy: no resyncs', fit.stats.resyncs === 0, fit.stats.resyncs);
  }

  // 11. Scroll and history.
  if (isReal) {
    await sleep(300);
    const before = screen(fit.grid);
    fit.scroll(10); // positive = up into history
    await waitFor('scrolled', () => screen(fit.grid) !== before, 3000).then(
      () => check('surface: scroll up changes view', true),
      () => check('surface: scroll up changes view', false),
    );
    fit.scrollToBottom();
    await waitFor('back at bottom', () => screen(fit.grid) === before, 3000).then(
      () => check('surface: scroll to bottom restores view', true),
      () => check('surface: scroll to bottom restores view', false),
    );
    const lines = await fit.history(200);
    check(
      'surface: history',
      lines.some((l) => l.includes('quick brown fox')),
      lines.slice(-3),
    );
  }

  // 12. Push register (fake token) and in-band notify while in the foreground.
  await client.registerPush('ExponentPushToken[smoke-test-not-real]', 'ios').then(
    () => check('push: register ok', true),
    (e) => check('push: register ok', false, String(e)),
  );
  client.setForeground(true);
  await sleep(200);
  if (target.setStatus) {
    await target.setStatus(pane, 'blocked');
    await waitFor('notify', () => notifies.find((n) => n.pane_id === pane && n.kind === 'blocked'), 8000).then(
      (n) => check('push: in-band notify for blocked agent', true, n),
      () => check('push: in-band notify for blocked agent', false, notifies),
    );
    console.log(`  notify: ${JSON.stringify(notifies[notifies.length - 1] ?? null)}`);
    await target.clearStatus?.(pane);
  }

  // 12b. Marking a done agent read turns it idle for every client.
  if (target.finish) {
    const agentOf = () => (agentsMsg as AgentsMessage | null)?.agents.find((a) => a.pane_id === pane);
    await target.finish(pane);
    const done = await waitFor('done agent', () => agentOf()?.agent_status === 'done' && agentOf(), 8000).catch(
      () => null,
    );
    if (!done) {
      console.log(
        `  skip mark_read: agent is ${agentOf()?.agent_status ?? 'missing'}, not done (is its pane on screen?)`,
      );
    } else {
      const seq = done.state_change_seq ?? 0;
      const stale = await client.request('pane.mark_read', { pane_id: pane, completion_seq: seq - 1 });
      await sleep(500);
      check('mark_read: an older completion leaves it done', !stale.error && agentOf()?.agent_status === 'done', stale);
      const read = await client.request('pane.mark_read', { pane_id: pane, completion_seq: seq });
      check('mark_read: ok', !read.error, read);
      await waitFor('read agent', () => agentOf()?.agent_status === 'idle', 8000).then(
        () => check('mark_read: agents watch shows idle', true),
        () => check('mark_read: agents watch shows idle', false, agentOf()),
      );
    }
    await target.clearStatus?.(pane);
  }
  await client.unregisterPush().then(
    () => check('push: unregister ok', true),
    (e) => check('push: unregister ok', false, String(e)),
  );
  unwatch();
  fit.unsubscribe();
  await sleep(300);

  // 13. Revoke closes the live connection, then the token is refused.
  if (target.revoke && !process.argv.includes('--measure-only')) {
    await target.revoke(creds.device_id);
    await waitFor('revoked close', () => fatal, 5000).then(
      () => check('revoke: live connection closed 4003', (fatal as unknown as { code: number }).code === 4003, fatal),
      () => check('revoke: live connection closed 4003', false, client.connStatus),
    );
    const after = newClient(payload, { kind: 'auth', device_id: creds.device_id, token: creds.token });
    let err = '';
    after.connect();
    try {
      await after.waitReady(5000);
    } catch (e) {
      err = (e as GatewayError).code;
    }
    check('revoke: token refused on reconnect', err === 'unauthorized' || err === 'revoked', err);
    after.close();
    if (target.devices) {
      const ids = await target.devices();
      check('revoke: device gone from list', !ids.includes(creds.device_id), ids);
    }
  }
  client.close();
  check('noise: every server frame was binary', textFrames === 0 && binaryFrames > 0, { binaryFrames, textFrames });

  console.log('\n--- measurements (45x30 fit) ---');
  console.log(`full frame: ${fitFrame ? `${fitFrame.list[0]}B` : 'n/a'}`);
  console.log(
    `typing a 12-char command: ${typeDeltas ? `${typeDeltas.count} deltas, avg ${Math.round(typeDeltas.bytes / typeDeltas.count)}B` : 'n/a'}`,
  );
  console.log(`one keystroke: ${oneChar}`);
  if (busy) console.log(busy);
  await target.stop();
  console.log(failures === 0 ? '\nSMOKE OK' : `\nSMOKE FAILED (${failures})`);
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
