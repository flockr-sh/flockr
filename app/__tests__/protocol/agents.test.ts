import { AgentsMessage, buildAgentRows, normalizeStatus } from '../../src/protocol';
import { fixture } from './fixtures';

describe('agent rows', () => {
  it('passes the spec agents message through', () => {
    const msg = fixture<AgentsMessage>('agents');
    const rows = buildAgentRows(msg);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ name: 'Claude', status: 'blocked', workspaceLabel: 'flockr', tabLabel: 'mobile' });
  });

  it('sorts blocked, done, working, idle, unknown, then workspace and tab number', () => {
    const base = { terminal_id: 't', workspace_id: 'w', tab_id: 'x' };
    const msg = {
      agents: [
        { ...base, pane_id: 'a', agent_status: 'idle' as const },
        { ...base, pane_id: 'b', agent_status: 'working' as const },
        { ...base, pane_id: 'c', agent_status: 'mystery' as never },
        { ...base, pane_id: 'd', agent_status: 'blocked' as const },
        { ...base, pane_id: 'e', agent_status: 'done' as const },
      ],
      workspaces: [],
      tabs: [],
    };
    expect(buildAgentRows(msg).map((r) => r.agent.pane_id)).toEqual(['d', 'e', 'b', 'a', 'c']);
    expect(normalizeStatus('later')).toBe('unknown');
  });
});
