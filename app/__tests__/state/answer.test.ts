import { readFileSync } from 'fs';
import { join } from 'path';
import { SHARED_FIXTURES } from '../sharedFixtures';
import { parsePrompt } from '../../src/protocol/prompt';
import { answerPrompt } from '../../src/state/answer';

const screen = readFileSync(join(SHARED_FIXTURES, 'prompts/claude-bash.txt'), 'utf8');

function fakeServer(status: string, text: string) {
  const calls: [string, Record<string, unknown>][] = [];
  const api = async <T>(method: string, params: Record<string, unknown> = {}): Promise<T> => {
    calls.push([method, params]);
    if (method === 'agent.get') return { type: 'agent', agent: { agent_status: status } } as T;
    if (method === 'agent.read') return { type: 'pane_read', read: { text } } as T;
    return { type: 'ok' } as T;
  };
  return { client: { api } as never, calls };
}

describe('answerPrompt', () => {
  const seen = parsePrompt(screen);
  if (!seen) throw new Error('fixture does not parse');

  it('re-reads the screen and sends only the digit', async () => {
    const { client, calls } = fakeServer('blocked', screen);
    expect(await answerPrompt(client, 'wE:p1', seen, seen.options[0])).toBe('sent');
    expect(calls).toEqual([
      ['agent.get', { target: 'wE:p1' }],
      ['agent.read', { target: 'wE:p1', source: 'detection', format: 'text' }],
      ['agent.send_keys', { target: 'wE:p1', keys: ['1'] }],
    ]);
  });

  it('sends nothing when the option under that number changed', async () => {
    const { client, calls } = fakeServer('blocked', screen.replace('4. No', '4. Yes, always'));
    expect(await answerPrompt(client, 'wE:p1', seen, seen.options[3])).toBe('changed');
    expect(calls.map(([m]) => m)).not.toContain('agent.send_keys');
  });

  it('sends nothing when the command changed', async () => {
    const { client, calls } = fakeServer(
      'blocked',
      screen.replace(' npm test 2>&1 | tail -100\n╌', ' rm -rf build\n╌'),
    );
    expect(await answerPrompt(client, 'wE:p1', seen, seen.options[0])).toBe('changed');
    expect(calls.map(([m]) => m)).not.toContain('agent.send_keys');
  });

  it('sends nothing when the prompt is gone', async () => {
    const { client, calls } = fakeServer('blocked', '● Done.\n');
    expect(await answerPrompt(client, 'wE:p1', seen, seen.options[0])).toBe('changed');
    expect(calls.map(([m]) => m)).not.toContain('agent.send_keys');
  });

  it('sends nothing to an agent that is not blocked', async () => {
    for (const status of ['working', 'idle', 'done', 'unknown']) {
      const { client, calls } = fakeServer(status, screen);
      expect(await answerPrompt(client, 'wE:p1', seen, seen.options[0])).toBe('not-blocked');
      expect(calls.map(([m]) => m)).toEqual(['agent.get']);
    }
  });
});
