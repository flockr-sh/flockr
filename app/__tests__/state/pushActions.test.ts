import { readFileSync } from 'fs';
import { join } from 'path';
import { SHARED_FIXTURES } from '../sharedFixtures';
import type { PushData } from '../../src/protocol';
import {
  answerFromPush,
  categoryActions,
  isPromptAction,
  parsePushPrompt,
  PROMPT_CATEGORIES,
  registerPromptCategories,
  type AnswerDeps,
} from '../../src/state/pushActions';

const screen = readFileSync(join(SHARED_FIXTURES, 'prompts/claude-bash.txt'), 'utf8');
const FINGERPRINT = '36dfc8675e9d416e241ddc54a4bb19d3';

// What the server sends for the claude-bash fixture (src/server/mobile/push.rs tests).
const pushData = (patch: Partial<PushData> = {}): PushData => ({
  type: 'agent_status',
  server_id: 'srv',
  status: 'blocked',
  agent: 'Claude',
  workspace: 'storefront',
  tab: 'checkout',
  pane_id: 'p_3',
  state_change_seq: 17,
  prompt: {
    category: 'prompt_yes_always_no',
    question: 'Do you want to proceed?',
    detail: 'npm test 2>&1 | tail -100',
    actions: {
      yes: { key: '1', label: 'Yes' },
      always: { key: '2', label: 'Yes, and don’t ask again for: npm test *' },
      no: { key: '4', label: 'No' },
    },
    fingerprint: FINGERPRINT,
  },
  ...patch,
});

// The same alert with `alert_prompt_text = false`: no prompt text at all.
const keysOnly = (patch: Partial<PushData> = {}): PushData =>
  pushData({
    prompt: {
      category: 'prompt_yes_always_no',
      actions: { yes: { key: '1' }, always: { key: '2' }, no: { key: '4' } },
      fingerprint: FINGERPRINT,
    },
    ...patch,
  });

function fakeServer(opts: { status?: string; seq?: number; text?: string; ready?: boolean } = {}) {
  const calls: [string, Record<string, unknown>][] = [];
  const api = async <T>(method: string, params: Record<string, unknown> = {}): Promise<T> => {
    calls.push([method, params]);
    if (method === 'agent.get')
      return { agent: { agent_status: opts.status ?? 'blocked', state_change_seq: opts.seq ?? 17 } } as T;
    if (method === 'agent.read') return { read: { text: opts.text ?? screen } } as T;
    return {} as T;
  };
  const waitReady = async () => {
    if (opts.ready === false) throw new Error('no welcome from server');
    return {} as never;
  };
  return { client: { api, waitReady }, calls };
}

function deps(client: ReturnType<typeof fakeServer>['client'] | null, demo = false) {
  const notes: string[] = [];
  const d: AnswerDeps = {
    client: () => client as never,
    isDemo: () => demo,
    notify: (title) => {
      notes.push(title);
    },
  };
  return { d, notes };
}

const sent = (calls: [string, Record<string, unknown>][]) =>
  calls.filter(([m]) => m === 'agent.send_keys').map(([, p]) => p.keys);

describe('notification categories', () => {
  it('registers the fixed set with Face ID on every answer', async () => {
    const registered: [string, ReturnType<typeof categoryActions>][] = [];
    await registerPromptCategories({
      setNotificationCategoryAsync: async (id, actions) => {
        registered.push([id, actions]);
      },
    });
    expect(registered.map(([id, a]) => [id, a.map((x) => x.buttonTitle)])).toEqual([
      ['prompt_yes_always_no', ['Yes', "Yes, don't ask again", 'No', 'Open']],
      ['prompt_yes_no', ['Yes', 'No', 'Open']],
    ]);
    for (const [, actions] of registered) {
      for (const a of actions) {
        expect(a.options.opensAppToForeground).toBe(true);
        expect(a.options.isAuthenticationRequired).toBe(a.identifier !== 'open');
      }
    }
  });

  it('uses ids expo-notifications accepts', () => {
    for (const c of PROMPT_CATEGORIES) expect(c.identifier).not.toMatch(/[:-]/);
  });

  it('knows which actions answer', () => {
    expect(['yes', 'always', 'no'].every(isPromptAction)).toBe(true);
    expect(isPromptAction('open')).toBe(false);
    expect(isPromptAction('expo.modules.notifications.actions.DEFAULT')).toBe(false);
  });
});

describe('parsePushPrompt', () => {
  it('maps each action to its option', () => {
    const p = parsePushPrompt(pushData().prompt);
    expect(p?.actions.yes).toEqual({ key: '1', label: 'Yes' });
    expect(p?.actions.always?.key).toBe('2');
    expect(p?.actions.no).toEqual({ key: '4', label: 'No' });
    expect(p?.detail).toBe('npm test 2>&1 | tail -100');
  });

  it('treats a null detail as none and drops malformed options', () => {
    const p = parsePushPrompt({
      category: 'prompt_yes_no',
      question: 'Continue?',
      detail: null,
      actions: { yes: { key: '1', label: 'Yes' }, no: { key: '12', label: 'No' } },
    });
    expect(p).toEqual({
      category: 'prompt_yes_no',
      question: 'Continue?',
      actions: { yes: { key: '1', label: 'Yes' } },
    });
  });

  it('reads a push without prompt text', () => {
    expect(parsePushPrompt(keysOnly().prompt)).toEqual({
      category: 'prompt_yes_always_no',
      fingerprint: FINGERPRINT,
      actions: { yes: { key: '1' }, always: { key: '2' }, no: { key: '4' } },
    });
  });

  it('rejects junk', () => {
    expect(parsePushPrompt(undefined)).toBeNull();
    expect(parsePushPrompt({ question: 'x' })).toBeNull();
    expect(parsePushPrompt({ category: 'c', question: '' })).toBeNull();
    // No text and no fingerprint: nothing to check the screen against.
    expect(parsePushPrompt({ category: 'c', actions: { yes: { key: '1' } } })).toBeNull();
    expect(parsePushPrompt({ category: 'c', fingerprint: 'abc', actions: {} })).toBeNull();
    expect(parsePushPrompt({ category: 'c', fingerprint: FINGERPRINT, detail: 'x', actions: {} })).toBeNull();
  });
});

describe('answerFromPush', () => {
  it.each([
    ['yes', '1', 'Answered checkout: Yes'],
    ['always', '2', "Answered checkout: Yes, don't ask again"],
    ['no', '4', 'Answered checkout: No'],
  ] as const)('%s re-checks the screen and sends %s', async (action, key, note) => {
    const server = fakeServer();
    const { d, notes } = deps(server.client);
    expect(await answerFromPush(d, pushData(), action)).toBe('sent');
    expect(server.calls).toEqual([
      ['agent.get', { target: 'p_3' }],
      ['agent.read', { target: 'p_3', source: 'detection', format: 'text' }],
      ['agent.send_keys', { target: 'p_3', keys: [key] }],
    ]);
    expect(notes).toEqual([note]);
  });

  it('answers a prompt without a detail', async () => {
    const text = readFileSync(join(SHARED_FIXTURES, 'prompts/claude-edit.txt'), 'utf8');
    const server = fakeServer({ text });
    const { d } = deps(server.client);
    const data = pushData({
      prompt: {
        category: 'prompt_yes_no',
        question: 'Do you want to make this edit to Cart.tsx?',
        detail: 'src/components/Cart.tsx',
        actions: {
          yes: { key: '1', label: 'Yes' },
          no: { key: '3', label: 'No, and tell Claude what to do differently' },
        },
      },
    });
    expect(await answerFromPush(d, data, 'no')).toBe('sent');
    expect(sent(server.calls)).toEqual([['3']]);
  });

  it('sends nothing when the agent is no longer blocked', async () => {
    const server = fakeServer({ status: 'working' });
    const { d, notes } = deps(server.client);
    expect(await answerFromPush(d, pushData(), 'yes')).toBe('not-blocked');
    expect(sent(server.calls)).toEqual([]);
    expect(notes).toEqual(["Couldn't answer checkout. Open flockr."]);
  });

  it('sends nothing when the agent blocked again since the alert', async () => {
    const server = fakeServer({ seq: 18 });
    const { d, notes } = deps(server.client);
    expect(await answerFromPush(d, pushData(), 'yes')).toBe('changed');
    expect(server.calls.map(([m]) => m)).toEqual(['agent.get']);
    expect(notes).toEqual(["Couldn't answer checkout. Open flockr."]);
  });

  it('sends nothing when the command changed', async () => {
    const server = fakeServer({ text: screen.replace(' npm test 2>&1 | tail -100\n╌', ' rm -rf build\n╌') });
    const { d, notes } = deps(server.client);
    expect(await answerFromPush(d, pushData(), 'yes')).toBe('changed');
    expect(sent(server.calls)).toEqual([]);
    expect(notes).toEqual(["Couldn't answer checkout. Open flockr."]);
  });

  it('sends nothing when the option under that number changed', async () => {
    const server = fakeServer({ text: screen.replace('4. No', '4. Yes, always') });
    const { d } = deps(server.client);
    expect(await answerFromPush(d, pushData(), 'no')).toBe('changed');
    expect(sent(server.calls)).toEqual([]);
  });

  it('fails without a server connection', async () => {
    const offline = fakeServer({ ready: false });
    const a = deps(offline.client);
    expect(await answerFromPush(a.d, pushData(), 'yes')).toBe('unreachable');
    expect(offline.calls).toEqual([]);
    expect(a.notes).toEqual(["Couldn't answer checkout. Open flockr."]);

    const b = deps(null);
    expect(await answerFromPush(b.d, pushData(), 'yes')).toBe('unreachable');
    expect(b.notes).toEqual(["Couldn't answer checkout. Open flockr."]);
  });

  it('fails without a prompt for that action, and on demo machines', async () => {
    const server = fakeServer();
    const { d, notes } = deps(server.client);
    expect(await answerFromPush(d, pushData({ prompt: undefined }), 'yes')).toBe('no-prompt');
    const yesNo = pushData();
    delete (yesNo.prompt as { actions: Record<string, unknown> }).actions.always;
    expect(await answerFromPush(d, yesNo, 'always')).toBe('no-prompt');
    expect(await answerFromPush(deps(server.client, true).d, pushData(), 'yes')).toBe('no-prompt');
    expect(server.calls).toEqual([]);
    expect(notes).toEqual(["Couldn't answer checkout. Open flockr.", "Couldn't answer checkout. Open flockr."]);
  });

  describe('without prompt text', () => {
    it.each([
      ['yes', '1'],
      ['always', '2'],
      ['no', '4'],
    ] as const)('%s sends %s when the fingerprint matches', async (action, key) => {
      const server = fakeServer();
      const { d } = deps(server.client);
      expect(await answerFromPush(d, keysOnly(), action)).toBe('sent');
      expect(sent(server.calls)).toEqual([[key]]);
    });

    it('still answers when only the cursor moved', async () => {
      const text = screen.replace('❯ 1. Yes', '  1. Yes').replace('   4. No', ' ❯ 4. No');
      const server = fakeServer({ text });
      expect(await answerFromPush(deps(server.client).d, keysOnly(), 'yes')).toBe('sent');
    });

    it.each([
      ['the command', screen.replace(' npm test 2>&1 | tail -100\n╌', ' rm -rf build\n╌')],
      ['an option the push has no button for', screen.replace('switch to auto mode ·', 'allow all commands ·')],
      ['the question', screen.replace('Do you want to proceed?', 'Do you want to deploy?')],
    ])('sends nothing when %s changed', async (_, text) => {
      const server = fakeServer({ text });
      const { d, notes } = deps(server.client);
      expect(await answerFromPush(d, keysOnly(), 'yes')).toBe('changed');
      expect(sent(server.calls)).toEqual([]);
      expect(notes).toEqual(["Couldn't answer checkout. Open flockr."]);
    });

    it('sends nothing when the agent blocked again or stopped', async () => {
      const again = fakeServer({ seq: 18 });
      expect(await answerFromPush(deps(again.client).d, keysOnly(), 'yes')).toBe('changed');
      const working = fakeServer({ status: 'working' });
      expect(await answerFromPush(deps(working.client).d, keysOnly(), 'yes')).toBe('not-blocked');
      expect([...sent(again.calls), ...sent(working.calls)]).toEqual([]);
    });

    it('sends nothing when the key is not on the screen', async () => {
      const server = fakeServer();
      const data = keysOnly({
        prompt: { category: 'prompt_yes_no', actions: { yes: { key: '1' }, no: { key: '7' } }, fingerprint: FINGERPRINT },
      });
      expect(await answerFromPush(deps(server.client).d, data, 'no')).toBe('changed');
      expect(sent(server.calls)).toEqual([]);
    });
  });

  it('still reports the answer when the note cannot be posted', async () => {
    const server = fakeServer();
    const d: AnswerDeps = {
      client: () => server.client as never,
      isDemo: () => false,
      notify: () => Promise.reject(new Error('no permission')),
    };
    expect(await answerFromPush(d, pushData(), 'yes')).toBe('sent');
  });
});
