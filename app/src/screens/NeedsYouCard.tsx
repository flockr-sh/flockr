// A blocked agent's permission prompt, answered with one tap: the question,
// the command it is about, and a button per option. Shown on the home screen
// and the desktop overview. When the screen does not parse as a prompt, the
// caller's usual card is shown instead.
import React, { useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from 'react-native';
import type { AgentRow, GatewayClient } from '../protocol';
import { isNoOption, shortLabel, type Prompt, type PromptOption } from '../protocol/prompt';
import { answerPrompt, useBlockedPrompt } from '../state/answer';
import { colors, mono, statusColor } from '../state/ui';

// How long "Sent" stays before the buttons come back, if the agent is still blocked.
const SENT_MS = 6000;

type Phase =
  | { kind: 'ready' }
  | { kind: 'sending'; key: string }
  | { kind: 'sent'; key: string }
  | { kind: 'note'; text: string };

interface Props {
  row: AgentRow;
  client: GatewayClient | null;
  serverId: string;
  /** Machine and workspace, e.g. "studio · storefront". */
  where: string;
  onOpen: () => void;
  /** Shown while there is no prompt to answer. */
  fallback: React.ReactNode;
}

export function NeedsYouCard({ row, client, serverId, where, onOpen, fallback }: Props) {
  const { prompt } = useBlockedPrompt(client, serverId, row);
  if (!client || !prompt) return <>{fallback}</>;
  return <PromptCard row={row} client={client} prompt={prompt} where={where} onOpen={onOpen} />;
}

function PromptCard({
  row,
  client,
  prompt,
  where,
  onOpen,
}: {
  row: AgentRow;
  client: GatewayClient;
  prompt: Prompt;
  where: string;
  onOpen: () => void;
}) {
  const [phase, setPhase] = useState<Phase>({ kind: 'ready' });
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);
  useEffect(() => {
    if (phase.kind !== 'sent') return;
    const t = setTimeout(() => live.current && setPhase({ kind: 'ready' }), SENT_MS);
    return () => clearTimeout(t);
  }, [phase]);

  const busy = phase.kind === 'sending' || phase.kind === 'sent';
  const answer = async (option: PromptOption) => {
    if (busy || row.status !== 'blocked') return;
    setPhase({ kind: 'sending', key: option.key });
    let next: Phase;
    try {
      const result = await answerPrompt(client, row.agent.pane_id, prompt, option);
      next =
        result === 'sent'
          ? { kind: 'sent', key: option.key }
          : result === 'changed'
            ? { kind: 'note', text: 'The prompt changed. Open the terminal.' }
            : { kind: 'note', text: 'Already answered.' };
    } catch {
      next = { kind: 'note', text: 'Could not send. Open the terminal.' };
    }
    if (live.current) setPhase(next);
  };

  return (
    <View style={styles.card} accessibilityLabel={`${row.name} needs you: ${prompt.question}`}>
      <View style={styles.head}>
        <View style={[styles.dot, { backgroundColor: statusColor.blocked }]} />
        <Text style={styles.name} numberOfLines={1}>
          {row.name}
        </Text>
        <Text style={styles.badge}>Needs you</Text>
      </View>
      {where ? (
        <Text style={styles.where} numberOfLines={1}>
          {where}
        </Text>
      ) : null}
      <Text style={styles.question}>{prompt.question}</Text>
      {prompt.detail ? (
        <View style={styles.detailBox}>
          <Text style={styles.detail} numberOfLines={3}>
            {prompt.detail}
          </Text>
        </View>
      ) : null}
      <View style={styles.options}>
        {prompt.options.map((o) => {
          const primary = o.key === '1';
          const no = isNoOption(o.label);
          const sending = phase.kind === 'sending' && phase.key === o.key;
          return (
            <Pressable
              key={o.key}
              accessibilityRole="button"
              accessibilityLabel={`${o.key}. ${o.label}`}
              accessibilityState={{ disabled: busy }}
              disabled={busy}
              onPress={() => void answer(o)}
              style={(state) => [
                styles.option,
                primary && !no && styles.primary,
                no && styles.no,
                ((state as { hovered?: boolean }).hovered || state.pressed) && !busy && styles.optionHover,
                busy && !sending && styles.disabled,
              ]}
            >
              {sending ? (
                <ActivityIndicator size="small" color={primary ? '#0b0d12' : colors.text} />
              ) : (
                <Text style={[styles.optionText, primary && !no && styles.primaryText]} numberOfLines={1}>
                  {shortLabel(o.label)}
                </Text>
              )}
            </Pressable>
          );
        })}
      </View>
      <View style={styles.foot}>
        <Text
          style={[styles.status, phase.kind === 'note' && { color: colors.warn }]}
          numberOfLines={2}
          accessibilityLiveRegion="polite"
        >
          {phase.kind === 'sent'
            ? `Sent: ${shortLabel(prompt.options.find((o) => o.key === phase.key)?.label ?? '', 28)}`
            : phase.kind === 'note'
              ? phase.text
              : ''}
        </Text>
        <Pressable accessibilityRole="link" onPress={onOpen} hitSlop={8}>
          <Text style={styles.open}>Open ›</Text>
        </Pressable>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  card: {
    padding: 14,
    borderRadius: 12,
    backgroundColor: colors.panel,
    borderWidth: 1,
    borderColor: '#5a2f3a',
    gap: 6,
  },
  head: { flexDirection: 'row', alignItems: 'center', gap: 10 },
  dot: { width: 10, height: 10, borderRadius: 5 },
  name: { color: colors.text, fontSize: 16, fontWeight: '600', flexShrink: 1 },
  badge: {
    marginLeft: 'auto',
    color: statusColor.blocked,
    fontSize: 11.5,
    fontWeight: '700',
    letterSpacing: 0.4,
    textTransform: 'uppercase',
  },
  where: { color: colors.dim, fontSize: 12, marginLeft: 20 },
  question: { color: colors.text, fontSize: 15, fontWeight: '500', marginTop: 6 },
  detailBox: {
    backgroundColor: colors.bg,
    borderRadius: 8,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: colors.border,
    paddingHorizontal: 10,
    paddingVertical: 8,
  },
  // No ligatures: a command must read exactly as typed ("--" not "–").
  detail: {
    color: colors.text,
    fontFamily: mono,
    fontSize: 12.5,
    fontVariant: ['no-common-ligatures', 'no-contextual'],
  },
  options: { flexDirection: 'row', flexWrap: 'wrap', gap: 8, marginTop: 6 },
  option: {
    flexGrow: 1,
    flexBasis: 220,
    minHeight: 40,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 12,
    paddingVertical: 9,
    borderRadius: 9,
    backgroundColor: colors.panelHi,
    borderWidth: 1,
    borderColor: colors.border,
  },
  optionHover: { opacity: 0.85 },
  primary: { backgroundColor: colors.accent, borderColor: colors.accent },
  primaryText: { color: '#0b0d12', fontWeight: '700' },
  no: { backgroundColor: 'transparent' },
  disabled: { opacity: 0.4 },
  optionText: { color: colors.text, fontSize: 14, fontWeight: '500' },
  foot: { flexDirection: 'row', alignItems: 'center', gap: 12, marginTop: 2, minHeight: 22 },
  status: { color: colors.ok, fontSize: 13, flex: 1 },
  open: { color: colors.accent, fontSize: 14, fontWeight: '600' },
});
