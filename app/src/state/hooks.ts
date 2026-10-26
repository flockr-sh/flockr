import { useEffect, useState, useSyncExternalStore } from 'react';
import {
  AgentRow,
  AgentsMessage,
  buildAgentRows,
  buildTerminalRows,
  ConnStatus,
  GatewayClient,
  PaneInfo,
  TerminalRow,
} from '../protocol';
import { connections } from './connections';
import type { StoredServer } from './storage';

export function useServers(): StoredServer[] {
  useSyncExternalStore(connections.subscribe, connections.getVersion);
  return connections.list();
}

export function useClientStatus(client: GatewayClient | null): ConnStatus {
  const [status, setStatus] = useState<ConnStatus>(client?.connStatus ?? { state: 'idle' });
  useEffect(() => {
    if (!client) return;
    setStatus(client.connStatus);
    return client.on('status', setStatus);
  }, [client]);
  return status;
}

interface AgentsState {
  rows: AgentRow[];
  loaded: boolean;
  /** The last `agents` message; its rev changes on every server event. */
  msg: AgentsMessage | null;
}

export function useAgents(client: GatewayClient | null): AgentsState {
  const [state, setState] = useState<AgentsState>({ rows: [], loaded: false, msg: null });
  useEffect(() => {
    if (!client) return;
    return client.watchAgents((msg) => setState({ rows: buildAgentRows(msg), loaded: true, msg }));
  }, [client]);
  return state;
}

/**
 * Shell panes, refreshed from `pane.list` whenever the agents watch reports a
 * change (the client allows one watcher, so this rides on useAgents).
 */
export function useTerminals(
  client: GatewayClient | null,
  msg: AgentsMessage | null,
): { rows: TerminalRow[]; loaded: boolean } {
  const [state, setState] = useState<{ rows: TerminalRow[]; loaded: boolean }>({ rows: [], loaded: false });
  useEffect(() => {
    if (!client || !msg) return;
    let live = true;
    client
      .api<{ panes?: PaneInfo[] }>('pane.list')
      .then((r) => live && setState({ rows: buildTerminalRows(r.panes ?? [], msg), loaded: true }))
      .catch(() => live && setState((s) => ({ ...s, loaded: true })));
    return () => {
      live = false;
    };
  }, [client, msg]);
  return state;
}

export function statusText(s: ConnStatus): string {
  switch (s.state) {
    case 'open':
      return 'connected';
    case 'connecting':
      return s.host ? `connecting to ${s.host.host}…` : 'connecting…';
    case 'waiting':
      return `offline: ${readable(s.error ?? 'closed')}, retrying in ${Math.ceil((s.retryInMs ?? 0) / 1000)}s`;
    case 'failed':
      return `failed: ${readable(s.error ?? 'unknown')}`;
    case 'closed':
      return 'closed';
    default:
      return 'idle';
  }
}

/** Error codes such as `connection_lost` as plain words. */
function readable(code: string): string {
  return code.replace(/_/g, ' ');
}
