// Agent list helpers: sorting and labels (section 9.3, 11.3).
import { AgentInfo, AgentStatus, AgentsMessage, TabInfo, WorkspaceInfo, normalizeStatus } from './types';

export interface AgentRow {
  agent: AgentInfo;
  status: AgentStatus;
  name: string;
  workspace?: WorkspaceInfo;
  tab?: TabInfo;
  workspaceLabel: string;
  tabLabel: string;
}

export function agentName(a: Pick<AgentInfo, 'display_agent' | 'name' | 'agent'>): string {
  return a.display_agent || a.name || a.agent || 'Agent';
}

export function buildAgentRows(msg: Pick<AgentsMessage, 'agents' | 'workspaces' | 'tabs'>): AgentRow[] {
  const ws = new Map(msg.workspaces.map((w) => [w.workspace_id, w]));
  const tabs = new Map(msg.tabs.map((t) => [t.tab_id, t]));
  const rows = msg.agents.map((agent): AgentRow => {
    const workspace = ws.get(agent.workspace_id);
    const tab = tabs.get(agent.tab_id);
    return {
      agent,
      status: normalizeStatus(agent.agent_status),
      name: agentName(agent),
      workspace,
      tab,
      workspaceLabel: workspace?.label || agent.workspace_id,
      tabLabel: tab?.label || agent.tab_id,
    };
  });
  // Sidebar order: by space, then tab.
  rows.sort(
    (a, b) =>
      (a.workspace?.number ?? 1e9) - (b.workspace?.number ?? 1e9) ||
      (a.tab?.number ?? 1e9) - (b.tab?.number ?? 1e9) ||
      a.agent.pane_id.localeCompare(b.agent.pane_id),
  );
  return rows;
}

/** Consecutive rows of the same space as list sections, keeping row order. */
export function groupBySpace<T extends { workspaceLabel: string }>(rows: T[]): { title: string; data: T[] }[] {
  const sections: { title: string; data: T[] }[] = [];
  for (const row of rows) {
    const last = sections[sections.length - 1];
    if (last && last.title === row.workspaceLabel) last.data.push(row);
    else sections.push({ title: row.workspaceLabel, data: [row] });
  }
  return sections;
}

/** A pane with no agent, from `pane.list`. */
export interface PaneInfo {
  pane_id: string;
  workspace_id: string;
  tab_id: string;
  agent?: string;
  cwd?: string;
  foreground_cwd?: string;
  terminal_title_stripped?: string;
  label?: string;
}

export interface TerminalRow {
  pane: PaneInfo;
  title: string;
  cwd: string;
  workspaceLabel: string;
  tabLabel: string;
}

/** Shell panes (no agent) labeled with their workspace and tab, in sidebar order. */
export function buildTerminalRows(panes: PaneInfo[], msg: Pick<AgentsMessage, 'workspaces' | 'tabs'>): TerminalRow[] {
  const ws = new Map(msg.workspaces.map((w) => [w.workspace_id, w]));
  const tabs = new Map(msg.tabs.map((t) => [t.tab_id, t]));
  return panes
    .filter((p) => !p.agent)
    .map((pane) => {
      const workspace = ws.get(pane.workspace_id);
      const tab = tabs.get(pane.tab_id);
      return {
        pane,
        title: pane.label || pane.terminal_title_stripped || 'Shell',
        cwd: pane.foreground_cwd || pane.cwd || '',
        workspaceLabel: workspace?.label || pane.workspace_id,
        tabLabel: tab?.label || pane.tab_id,
        order: [workspace?.number ?? 1e9, tab?.number ?? 1e9] as const,
      };
    })
    .sort((a, b) => a.order[0] - b.order[0] || a.order[1] - b.order[1] || a.pane.pane_id.localeCompare(b.pane.pane_id))
    .map(({ order: _order, ...row }) => row);
}
