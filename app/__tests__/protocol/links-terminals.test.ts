import { buildTerminalRows, emptyGrid, Grid, linkAt } from '../../src/protocol';

function grid(lines: Grid['lines'], extra: Partial<Grid> = {}): Grid {
  return { ...emptyGrid(), cols: 40, rows: lines.length, seq: 1, lines, styles: [[-1, -1, 0]], ...extra };
}

describe('linkAt', () => {
  it('finds a plain-text URL under the column and drops trailing punctuation', () => {
    const g = grid([[[0, 0, 'see https://flockr.sh/docs. ok']]]);
    expect(linkAt(g, 4, 0)).toBe('https://flockr.sh/docs');
    expect(linkAt(g, 25, 0)).toBe('https://flockr.sh/docs');
    expect(linkAt(g, 26, 0)).toBeNull();
    expect(linkAt(g, 1, 0)).toBeNull();
  });

  it('prefers an OSC 8 link from the style table', () => {
    const g = grid(
      [
        [
          [0, 0, 'go '],
          [3, 1, 'here'],
        ],
      ],
      {
        styles: [
          [-1, -1, 0],
          [-1, -1, 8, 0],
        ],
        links: ['https://example.com/x'],
      },
    );
    expect(linkAt(g, 4, 0)).toBe('https://example.com/x');
    expect(linkAt(g, 0, 0)).toBeNull();
  });

  it('returns null outside the grid', () => {
    const g = grid([[[0, 0, 'https://a.b']]]);
    expect(linkAt(g, 2, 5)).toBeNull();
    expect(linkAt(g, -1, 0)).toBeNull();
  });
});

describe('buildTerminalRows', () => {
  it('keeps panes without an agent, labeled and in workspace/tab order', () => {
    const rows = buildTerminalRows(
      [
        { pane_id: 'w2:p1', workspace_id: 'w2', tab_id: 'w2:t1', terminal_title_stripped: 'vim' },
        { pane_id: 'w1:p1', workspace_id: 'w1', tab_id: 'w1:t1', agent: 'claude' },
        { pane_id: 'w1:p2', workspace_id: 'w1', tab_id: 'w1:t1', cwd: '/tmp' },
      ],
      {
        workspaces: [
          { workspace_id: 'w1', label: 'flockr', number: 1 },
          { workspace_id: 'w2', label: 'notes', number: 2 },
        ],
        tabs: [
          { tab_id: 'w1:t1', label: 'main', number: 1 },
          { tab_id: 'w2:t1', label: '1', number: 1 },
        ],
      } as never,
    );
    expect(rows.map((r) => r.pane.pane_id)).toEqual(['w1:p2', 'w2:p1']);
    expect(rows[0]).toMatchObject({ title: 'Shell', cwd: '/tmp', workspaceLabel: 'flockr', tabLabel: 'main' });
    expect(rows[1]).toMatchObject({ title: 'vim', workspaceLabel: 'notes' });
  });
});
