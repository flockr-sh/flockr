import {
  AgentsMessage,
  agentDetail,
  agentMarker,
  buildAgentRows,
  groupBySpace,
  normalizeStatus,
  shortBranch,
} from '../../src/protocol';
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

describe('agent row markers and detail', () => {
  const msg = (agent: Record<string, unknown>, tabLabel = '2') =>
    ({
      agents: [{ terminal_id: 't', pane_id: 'p', workspace_id: 'w', tab_id: 'tb', agent_status: 'idle', ...agent }],
      workspaces: [{ workspace_id: 'w', number: 1, label: 'flockr' }],
      tabs: [{ tab_id: 'tb', workspace_id: 'w', number: 2, label: tabLabel }],
    }) as unknown as AgentsMessage;

  it('marks agents like the sidebar Dots style', () => {
    expect(agentMarker('working', false)).toBe('pulse');
    expect(agentMarker('blocked', true)).toBe('dot');
    expect(agentMarker('done', false)).toBe('dot');
    expect(agentMarker('idle', false)).toBe('ring');
    expect(agentMarker('idle', true)).toBe('dotted-ring');
    expect(agentMarker('working', true)).toBe('pulse');
    expect(agentMarker('unknown', false)).toBe('faint');
  });

  it('reads branch and session_empty and builds the detail line', () => {
    const [row] = buildAgentRows(
      msg({ display_agent: 'Claude', name: 'reviewer', branch: 'worktree/fix-login', session_empty: true }),
    );
    expect(row).toMatchObject({ branch: 'fix-login', sessionEmpty: true });
    expect(agentDetail(row)).toBe('tab 2 · reviewer · fix-login');
  });

  it('skips missing parts and a name the first line already shows', () => {
    const [row] = buildAgentRows(msg({ name: 'reviewer' }, 'mobile'));
    expect(row).toMatchObject({ name: 'reviewer', branch: undefined, sessionEmpty: false });
    expect(agentDetail(row)).toBe('tab 2 mobile');
    expect(shortBranch('main')).toBe('main');
  });
});
