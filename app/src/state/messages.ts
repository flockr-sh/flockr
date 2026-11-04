// Messages between agents that wait for a person to approve them, on every
// machine, and each machine's approval setting. Servers in a flock offer
// these through the `messages` feature (protocol section 9.2).
import { useEffect, useSyncExternalStore } from 'react';
import type { GatewayClient } from '../protocol';
import { connections } from './connections';

export type ApproveMode = 'other-projects' | 'never' | 'always';

export const APPROVE_MODES: { mode: ApproveMode; label: string; help: string }[] = [
  {
    mode: 'other-projects',
    label: 'Other projects',
    help: 'Ask when an agent messages an agent in a different repo. Replies and agents in the same repo go through.',
  },
  { mode: 'never', label: 'Never', help: 'Every message from an agent goes through.' },
  { mode: 'always', label: 'Always', help: 'Ask for every message from an agent, unless you allowed the pair.' },
];

export interface MessageParty {
  member: string;
  machine: string;
  agent?: string | null;
}

export interface PendingMessage {
  id: string;
  conversation: string;
  from: MessageParty;
  to: MessageParty;
  body: string;
  sent_at: number;
}

export interface Waiting {
  serverId: string;
  message: PendingMessage;
}

const POLL_MS = 5000;

/** Who sent or gets a message, as `machine:agent`. */
export function partyLabel(party: MessageParty): string {
  return party.agent ? `${party.machine}:${party.agent}` : `${party.machine} (a person)`;
}

/** The machine's client when it is connected and takes message methods. */
export function messagesClient(serverId: string): GatewayClient | null {
  const client = connections.client(serverId);
  return client && client.connStatus.state === 'open' && client.hasFeature('messages') ? client : null;
}

// One poll for every screen that shows waiting messages.
let waiting: Waiting[] = [];
let version = 0;
let timer: ReturnType<typeof setInterval> | null = null;
const listeners = new Set<() => void>();

function publish(next: Waiting[]) {
  const key = (list: Waiting[]) => list.map((w) => `${w.serverId}/${w.message.id}`).join(',');
  if (key(next) === key(waiting)) return;
  waiting = next;
  version += 1;
  for (const listener of Array.from(listeners)) listener();
}

export async function refreshWaiting(): Promise<void> {
  const lists = await Promise.all(
    connections.list().map(async (server) => {
      const client = messagesClient(server.server_id);
      if (!client) return [];
      try {
        const result = await client.api<{ pending?: PendingMessage[] }>('message.pending');
        return (result.pending ?? []).map((message) => ({ serverId: server.server_id, message }));
      } catch {
        return [];
      }
    }),
  );
  publish(lists.flat().sort((a, b) => a.message.sent_at - b.message.sent_at));
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (!timer) {
    void refreshWaiting();
    timer = setInterval(() => void refreshWaiting(), POLL_MS);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && timer) {
      clearInterval(timer);
      timer = null;
    }
  };
}

/** Messages waiting for approval on every connected machine, oldest first. */
export function useWaitingMessages(): Waiting[] {
  useSyncExternalStore(subscribe, () => version);
  // A machine that connects later is polled from then on; refresh when the
  // server list changes so it shows up without waiting a full poll.
  useEffect(() => connections.subscribe(() => void refreshWaiting()), []);
  return waiting;
}

export async function approveMessage(serverId: string, id: string, always: boolean): Promise<void> {
  const client = messagesClient(serverId);
  if (!client) throw new Error('that machine is not connected');
  await client.api('message.approve', { id, always });
  await refreshWaiting();
}

export async function denyMessage(serverId: string, id: string): Promise<void> {
  const client = messagesClient(serverId);
  if (!client) throw new Error('that machine is not connected');
  await client.api('message.deny', { id });
  await refreshWaiting();
}

export async function loadApproveMode(serverId: string): Promise<ApproveMode> {
  const client = messagesClient(serverId);
  if (!client) throw new Error('not connected');
  const result = await client.api<{ approve_messages?: ApproveMode }>('message.settings');
  return result.approve_messages ?? 'other-projects';
}

export async function saveApproveMode(serverId: string, mode: ApproveMode): Promise<ApproveMode> {
  const client = messagesClient(serverId);
  if (!client) throw new Error('not connected');
  const result = await client.api<{ approve_messages?: ApproveMode }>('message.settings.set', {
    approve_messages: mode,
  });
  return result.approve_messages ?? mode;
}
