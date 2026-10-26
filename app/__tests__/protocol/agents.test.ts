import { AgentsMessage, buildAgentRows, groupBySpace, normalizeStatus } from '../../src/protocol';
import { fixture } from './fixtures';

describe('agent rows', () => {
  it('passes the spec agents message through', () => {
    const msg = fixture<AgentsMessage>('agents');
    const rows = buildAgentRows(msg);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ name: 'Claude', status: 'blocked', workspaceLabel: 'flockr', tabLabel: 'mobile' });
  });

  it('sorts in sidebar order (space, then tab) and groups by space', () => {
    const msg = {
      agents: [
        { terminal_id: 't', pane_id: 'a', workspace_id: 'w2', tab_id: 'w2:t1', agent_status: 'blocked' as const },
        { terminal_id: 't', pane_id: 'b', workspace_id: 'w1', tab_id: 'w1:t2', agent_status: 'idle' as const },
        { terminal_id: 't', pane_id: 'c', workspace_id: 'w1', tab_id: 'w1:t1', agent_status: 'mystery' as never },
      ],
      workspaces: [
        { workspace_id: 'w1', number: 1, label: 'Dotfiles' },
        { workspace_id: 'w2', number: 2, label: 'flockr' },
      ],
      tabs: [
        { tab_id: 'w1:t1', workspace_id: 'w1', number: 1, label: '1' },
        { tab_id: 'w1:t2', workspace_id: 'w1', number: 2, label: '2' },
        { tab_id: 'w2:t1', workspace_id: 'w2', number: 1, label: '1' },
      ],
    } as unknown as AgentsMessage;
    const rows = buildAgentRows(msg);
    expect(rows.map((r) => r.agent.pane_id)).toEqual(['c', 'b', 'a']);
    expect(groupBySpace(rows).map((g) => [g.title, g.data.map((r) => r.agent.pane_id)])).toEqual([
      ['Dotfiles', ['c', 'b']],
      ['flockr', ['a']],
    ]);
    expect(normalizeStatus('later')).toBe('unknown');
  });
});
