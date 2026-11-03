import type { AgentInfo } from '../protocol';

/** How long a done agent's terminal stays on screen before the phone marks it read. */
export const MARK_READ_DELAY_MS = 1500;

/** Errors that mean the request never reached the server, so the read is retried. */
const TRANSPORT_ERRORS = new Set(['not_connected', 'timeout', 'closed', 'disconnected']);

export function isTransportError(code: string): boolean {
  return TRANSPORT_ERRORS.has(code);
}

/**
 * The completion to mark read for an open terminal, or null when there is
 * nothing to do: the agent is not done, the app is in the background, or this
 * completion was already marked.
 */
export function completionToMarkRead(
  agent: AgentInfo | undefined,
  foreground: boolean,
  lastMarked: number | null,
): number | null {
  if (!agent || !foreground || agent.agent_status !== 'done') return null;
  const seq = agent.state_change_seq;
  if (seq === undefined || seq === lastMarked) return null;
  return seq;
}
