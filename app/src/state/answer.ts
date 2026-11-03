// Answering a blocked agent's permission prompt from the home screen: read the
// bottom of its screen while it is blocked, and on a tap check the prompt is
// still the one the person saw before sending the option's digit.
import { useEffect, useState } from 'react';
import { normalizeStatus, type AgentRow, type GatewayClient } from '../protocol';
import { parsePrompt, samePrompt, type Prompt, type PromptOption } from '../protocol/prompt';

/** How often a blocked agent's screen is read again while it stays blocked. */
export const PROMPT_POLL_MS = 4000;

type Api = Pick<GatewayClient, 'api'>;

/** The bottom of an agent's screen as plain text. */
export async function readScreen(client: Api, paneId: string): Promise<string> {
  const r = await client.api<{ read?: { text?: string } }>('agent.read', {
    target: paneId,
    source: 'detection',
    format: 'text',
  });
  return r.read?.text ?? '';
}

export async function readPrompt(client: Api, paneId: string): Promise<Prompt | null> {
  return parsePrompt(await readScreen(client, paneId));
}

export type AnswerResult = 'sent' | 'changed' | 'not-blocked';

/**
 * Send `option`'s digit to the agent, but only if it is still blocked and its
 * screen still shows the same prompt with the same option under that number.
 * `opts.stateChangeSeq` also requires the agent's `state_change_seq` to match.
 */
export async function answerPrompt(
  client: Api,
  paneId: string,
  seen: Prompt,
  option: PromptOption,
  opts: { stateChangeSeq?: number } = {},
): Promise<AnswerResult> {
  const got = await client.api<{ agent?: { agent_status?: string; state_change_seq?: number } }>('agent.get', {
    target: paneId,
  });
  if (normalizeStatus(got.agent?.agent_status) !== 'blocked') return 'not-blocked';
  // With a seq (from a push), the agent must still be in the same blocked spell.
  if (opts.stateChangeSeq !== undefined && got.agent?.state_change_seq !== opts.stateChangeSeq) return 'changed';
  const now = await readPrompt(client, paneId);
  const same = now?.options.find((o) => o.key === option.key);
  if (!now || !samePrompt(now, seen) || !same || same.label !== option.label) return 'changed';
  // Claude Code and Codex pick an option on its digit; no Enter.
  await client.api('agent.send_keys', { target: paneId, keys: [option.key] });
  return 'sent';
}

// Last prompt read per server and pane, so a card shows it again at once
// when it remounts (switching screens or layouts).
const cache = new Map<string, Prompt | null>();

export function promptKey(serverId: string, paneId: string): string {
  return `${serverId}\u0000${paneId}`;
}

export interface PromptState {
  prompt: Prompt | null;
  /** True once the screen was read at least once while blocked. */
  read: boolean;
}

/**
 * The permission prompt a blocked agent shows. Reads only while the agent is
 * blocked: when it becomes blocked and every few seconds after.
 */
export function useBlockedPrompt(client: GatewayClient | null, serverId: string, row: AgentRow): PromptState {
  const key = promptKey(serverId, row.agent.pane_id);
  const blocked = row.status === 'blocked';
  const [state, setState] = useState<PromptState>(() =>
    cache.has(key) ? { prompt: cache.get(key) ?? null, read: true } : { prompt: null, read: false },
  );
  const seq = row.agent.state_change_seq;
  useEffect(() => {
    if (!client || !blocked) {
      cache.delete(key);
      setState({ prompt: null, read: false });
      return;
    }
    let live = true;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const poll = async () => {
      try {
        const prompt = await readPrompt(client, row.agent.pane_id);
        if (!live) return;
        cache.set(key, prompt);
        setState((prev) => (prev.read && sameShown(prev.prompt, prompt) ? prev : { prompt, read: true }));
      } catch {
        // Offline or not allowed: keep what we had; the card still opens the terminal.
        if (live) setState((prev) => (prev.read ? prev : { prompt: null, read: true }));
      }
      if (live) timer = setTimeout(() => void poll(), PROMPT_POLL_MS);
    };
    void poll();
    return () => {
      live = false;
      if (timer) clearTimeout(timer);
    };
  }, [client, blocked, key, seq, row.agent.pane_id]);
  return blocked ? state : { prompt: null, read: false };
}

function sameShown(a: Prompt | null, b: Prompt | null): boolean {
  if (!a || !b) return a === b;
  return (
    samePrompt(a, b) &&
    a.selected === b.selected &&
    a.options.length === b.options.length &&
    a.options.every((o, i) => o.key === b.options[i].key && o.label === b.options[i].label)
  );
}
