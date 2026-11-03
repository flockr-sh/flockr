// Answer buttons on "agent is blocked" push alerts. The server picks one of
// the categories below when the prompt's options map cleanly (Yes / Yes,
// don't ask again / No) and puts the prompt it saw in `data.prompt`.
//
// iOS gives an app no reliable background time for an action response:
// expo-notifications only delivers it to JS when the app is running, and
// completes it at once, so a suspended app would have to open the gateway
// connection, run the Noise handshake, and make three API calls with no
// time granted. So every action opens the app (and needs Face ID or the
// passcode first); the app answers as soon as it is up.
//
// Plain module with no native imports so it runs under jest and on web.
import type { GatewayClient, PushData } from '../protocol';
import type { Prompt, PromptOption } from '../protocol/prompt';
import { answerPrompt, type AnswerResult } from './answer';

export type PromptAction = 'yes' | 'always' | 'no';
export const OPEN_ACTION = 'open';

export const ACTION_TITLES: Record<PromptAction | typeof OPEN_ACTION, string> = {
  yes: 'Yes',
  always: "Yes, don't ask again",
  no: 'No',
  open: 'Open',
};

/** The fixed set the server chooses from (`src/server/mobile/prompt.rs`). */
export const PROMPT_CATEGORIES: { identifier: string; actions: (PromptAction | typeof OPEN_ACTION)[] }[] = [
  { identifier: 'prompt_yes_always_no', actions: ['yes', 'always', 'no', 'open'] },
  { identifier: 'prompt_yes_no', actions: ['yes', 'no', 'open'] },
];

/** The shape `setNotificationCategoryAsync` takes for one action. */
export interface CategoryAction {
  identifier: string;
  buttonTitle: string;
  options: { opensAppToForeground: boolean; isAuthenticationRequired: boolean };
}

export function categoryActions(ids: (PromptAction | typeof OPEN_ACTION)[]): CategoryAction[] {
  return ids.map((id) => ({
    identifier: id,
    buttonTitle: ACTION_TITLES[id],
    // Answers need the phone unlocked so a locked phone can't approve.
    // Open foregrounds the app, which iOS only does once unlocked anyway.
    options: { opensAppToForeground: true, isAuthenticationRequired: id !== OPEN_ACTION },
  }));
}

export interface CategoryRegistrar {
  setNotificationCategoryAsync(identifier: string, actions: CategoryAction[]): Promise<unknown>;
}

export async function registerPromptCategories(N: CategoryRegistrar): Promise<void> {
  for (const c of PROMPT_CATEGORIES) await N.setNotificationCategoryAsync(c.identifier, categoryActions(c.actions));
}

export function isPromptAction(id: string): id is PromptAction {
  return id === 'yes' || id === 'always' || id === 'no';
}

/** What the server saw when it sent the alert. */
export interface PushPrompt {
  category: string;
  question: string;
  detail?: string;
  actions: Partial<Record<PromptAction, PromptOption>>;
}

function option(value: unknown): PromptOption | undefined {
  if (!value || typeof value !== 'object') return undefined;
  const o = value as Record<string, unknown>;
  if (typeof o.key !== 'string' || !/^\d$/.test(o.key) || typeof o.label !== 'string' || !o.label) return undefined;
  return { key: o.key, label: o.label };
}

export function parsePushPrompt(value: unknown): PushPrompt | null {
  if (!value || typeof value !== 'object') return null;
  const p = value as Record<string, unknown>;
  if (typeof p.category !== 'string' || typeof p.question !== 'string' || !p.question) return null;
  if (p.detail != null && typeof p.detail !== 'string') return null;
  const raw = (p.actions && typeof p.actions === 'object' ? p.actions : {}) as Record<string, unknown>;
  const actions: PushPrompt['actions'] = {};
  for (const id of ['yes', 'always', 'no'] as const) {
    const o = option(raw[id]);
    if (o) actions[id] = o;
  }
  return {
    category: p.category,
    question: p.question,
    ...(typeof p.detail === 'string' ? { detail: p.detail } : {}),
    actions,
  };
}

/** The tab (or workspace, or agent) an alert is about, for the result note. */
export function placeName(d: PushData): string {
  return d.tab || d.workspace || d.agent || 'the agent';
}

export interface AnswerDeps {
  /** Connected (or connecting) client for a paired server, or null. */
  client(serverId: string): Pick<GatewayClient, 'api' | 'waitReady'> | null;
  /** Demo machines have no server to answer. */
  isDemo(serverId: string): boolean;
  /** Post a quiet local notification. */
  notify(title: string, data: PushData): Promise<void> | void;
  connectTimeoutMs?: number;
}

export type PushAnswer = AnswerResult | 'no-prompt' | 'unreachable';

/**
 * Answer a prompt from an alert's button. Sends keys only through
 * `answerPrompt`, which re-checks the agent is still blocked (in the same
 * status change) and re-reads its screen. Posts the result either way.
 */
export async function answerFromPush(deps: AnswerDeps, data: PushData, action: PromptAction): Promise<PushAnswer> {
  const prompt = parsePushPrompt(data.prompt);
  const chosen = prompt?.actions[action];
  let result: PushAnswer;
  if (!prompt || !chosen || deps.isDemo(data.server_id)) {
    result = 'no-prompt';
  } else {
    const client = deps.client(data.server_id);
    if (!client) {
      result = 'unreachable';
    } else {
      const seen: Prompt = {
        question: prompt.question,
        ...(prompt.detail !== undefined ? { detail: prompt.detail } : {}),
        options: [chosen],
      };
      try {
        await client.waitReady(deps.connectTimeoutMs ?? 15000);
        result = await answerPrompt(client, data.pane_id, seen, chosen, {
          stateChangeSeq: typeof data.state_change_seq === 'number' ? data.state_change_seq : undefined,
        });
      } catch {
        result = 'unreachable';
      }
    }
  }
  const name = placeName(data);
  try {
    await deps.notify(
      result === 'sent' ? `Answered ${name}: ${ACTION_TITLES[action]}` : `Couldn't answer ${name}. Open flockr.`,
      data,
    );
  } catch {
    // The answer (or refusal) stands without the note.
  }
  return result;
}
