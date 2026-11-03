import { DEMO_DEVICE_ID, DEMO_HUB_ID, DEMO_TOKEN, DemoFlock } from '../../src/demo/flock';
import { AgentInfo, decodeServerKey, GatewayClient, gridText, Subscription } from '../../src/protocol';

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

describe('demo flock', () => {
  let flock: DemoFlock;
  const clients: GatewayClient[] = [];

  beforeEach(() => {
    flock = new DemoFlock({ ticker: false });
  });

  afterEach(() => {
    for (const c of clients.splice(0)) c.close();
    flock.stop();
  });

  async function connect(machine?: string): Promise<GatewayClient> {
    const key = decodeServerKey(flock.pk);
    if (!key) throw new Error('bad pk');
    const c = new GatewayClient({
      hosts: [{ host: 'demo', port: 0 }],
      sid: DEMO_HUB_ID,
      machine,
      serverKey: key,
      credentials: { kind: 'auth', device_id: DEMO_DEVICE_ID, token: DEMO_TOKEN },
      app: { version: 'test', platform: 'web' },
      createSocket: flock.createSocket,
      reconnect: false,
      pingIntervalMs: 0,
    });
    clients.push(c);
    c.connect();
    await c.waitReady(3000);
    return c;
  }

  async function agents(c: GatewayClient): Promise<AgentInfo[]> {
    const r = await c.api<{ agents: AgentInfo[] }>('agent.list');
    return r.agents;
  }

  async function screen(c: GatewayClient, paneId: string): Promise<{ sub: Subscription; text: () => string }> {
    const sub = c.subscribe(paneId, { mode: 'fit', cols: 60, rows: 30 });
    await waitFor(() => sub.grid.seq > 0);
    return { sub, text: () => gridText(sub.grid).join('\n') };
  }

  it('lists the hub and its second machine with their agents', async () => {
    const studio = await connect();
    expect(studio.welcome?.server.name).toBe('studio');
    expect(studio.welcome?.machines?.map((m) => [m.name, m.this])).toEqual([
      ['studio', true],
      ['build-box', false],
    ]);
    expect(studio.hasFeature('messages')).toBe(true);
    expect(studio.hasFeature('push.expo')).toBe(false);
    const hubAgents = await agents(studio);
    expect(hubAgents.map((a) => [a.name, a.agent_status, a.terminal_title_stripped])).toEqual([
      ['checkout', 'blocked', 'Storefront test suite'],
      ['api', 'working', 'Per-customer rate limiting for /invoices'],
      ['infra-review', 'done', 'Review billing Postgres risks'],
    ]);

    const buildBoxId = studio.welcome?.machines?.find((m) => !m.this)?.id;
    const buildBox = await connect(buildBoxId);
    expect(buildBox.welcome?.server.name).toBe('build-box');
    expect(buildBox.welcome?.server.server_id).toBe(DEMO_HUB_ID);
    expect((await agents(buildBox)).map((a) => [a.name, a.agent_status])).toEqual([
      ['tests', 'done'],
      ['storefront-ui', 'done'],
    ]);
  });

  it('approves the waiting message and the agents talk', async () => {
    const studio = await connect();
    const { pending } = await studio.api<{ pending: { id: string; from: { agent: string }; body: string }[] }>(
      'message.pending',
    );
    expect(pending).toHaveLength(1);
    expect(pending[0].from.agent).toBe('storefront-ui');
    expect(pending[0].body).toContain('SAVE10');

    const api = await screen(studio, 'p_api');
    await studio.api('message.approve', { id: pending[0].id, always: false });
    expect((await studio.api<{ pending: unknown[] }>('message.pending')).pending).toHaveLength(0);
    flock.advance(1000);
    await waitFor(() => api.text().includes('Message from build-box:storefront-ui'));
    // The agent keeps working.
    expect((await agents(studio)).find((a) => a.name === 'api')?.agent_status).toBe('working');
    flock.advance(2000);
    expect(flock.pane('m_buildbox', 'p_storefront_ui')?.history().join('\n')).toContain('Reply from studio:api');
  });

  it('changes the approval setting in memory', async () => {
    const studio = await connect();
    expect((await studio.api('message.settings')).approve_messages).toBe('other-projects');
    await studio.api('message.settings.set', { approve_messages: 'always' });
    expect((await studio.api('message.settings')).approve_messages).toBe('always');
    await expect(studio.api('message.settings.set', { approve_messages: 'sometimes' })).rejects.toThrow();
  });

  it('resolves the checkout permission prompt', async () => {
    const studio = await connect();
    const checkout = await screen(studio, 'p_checkout');
    expect(checkout.text()).toContain('Do you want to proceed?');
    expect(checkout.text()).toContain('4. No');

    await checkout.sub.input('text', '1');
    await waitFor(() => !checkout.text().includes('Do you want to proceed?'));
    expect((await agents(studio)).find((a) => a.name === 'checkout')?.agent_status).toBe('working');

    for (let i = 0; i < 20; i++) flock.advance(500);
    await waitFor(() => checkout.text().includes('12 passed'));
    expect((await agents(studio)).find((a) => a.name === 'checkout')?.agent_status).toBe('done');
  });

  it('answers a typed prompt without running anything', async () => {
    const studio = await connect();
    const infra = await screen(studio, 'p_infra');
    await infra.sub.input('submit', 'draft the terraform changes');
    await waitFor(() => infra.text().includes('draft the terraform changes'));
    flock.advance(3000);
    await waitFor(() => infra.text().includes('This is a demo machine'));
  });
});
