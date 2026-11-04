import type { AgentInfo } from '../../src/protocol';
import { completionToMarkRead, isTransportError, MARK_READ_DELAY_MS } from '../../src/state/markRead';

const agent = (agent_status: AgentInfo['agent_status'], state_change_seq?: number): AgentInfo => ({
  terminal_id: 't1',
  pane_id: 'w1:p1',
  workspace_id: 'w1',
  tab_id: 'w1:t1',
  agent_status,
  state_change_seq,
});

describe('mark read', () => {
  it('marks a done agent read while the app is in the foreground', () => {
    expect(completionToMarkRead(agent('done', 7), true, null)).toBe(7);
  });

  it('leaves agents that are not done alone', () => {
    for (const status of ['idle', 'working', 'blocked', 'unknown'] as const)
      expect(completionToMarkRead(agent(status, 7), true, null)).toBeNull();
    expect(completionToMarkRead(undefined, true, null)).toBeNull();
  });

  it('waits while the app is in the background', () => {
    expect(completionToMarkRead(agent('done', 7), false, null)).toBeNull();
  });

  it('marks each completion once', () => {
    expect(completionToMarkRead(agent('done', 7), true, 7)).toBeNull();
    expect(completionToMarkRead(agent('done', 9), true, 7)).toBe(9);
  });

  it('needs the completion sequence', () => {
    expect(completionToMarkRead(agent('done'), true, null)).toBeNull();
  });

  it('retries only when the request never reached the server', () => {
    expect(isTransportError('not_connected')).toBe(true);
    expect(isTransportError('method_not_allowed')).toBe(false);
    expect(MARK_READ_DELAY_MS).toBeGreaterThan(0);
  });
});
