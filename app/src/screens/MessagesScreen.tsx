// Messages between agents that wait for you, from every machine, with
// Approve, Always (approve and allow the pair from now on), and Deny.
import React, { useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import type { Nav } from '../state/nav';
import { approveMessage, denyMessage, partyLabel, useWaitingMessages, type Waiting } from '../state/messages';
import { colors, ui } from '../state/ui';

export function MessagesScreen({ nav }: { nav: Nav }) {
  return (
    <SafeAreaView style={ui.screen}>
      <View style={ui.header}>
        <Pressable accessibilityRole="button" style={ui.button} onPress={() => nav.pop()}>
          <Text style={ui.buttonText}>‹</Text>
        </Pressable>
        <Text style={[ui.title, { flex: 1 }]}>Messages</Text>
      </View>
      <ScrollView contentContainerStyle={{ padding: 12 }}>
        <MessagesList />
      </ScrollView>
    </SafeAreaView>
  );
}

export function MessagesList() {
  const waiting = useWaitingMessages();
  if (waiting.length === 0) {
    return (
      <Text style={styles.empty}>
        No messages are waiting. When an agent messages an agent in another project, it waits here for you.
      </Text>
    );
  }
  return (
    <View style={{ gap: 12 }}>
      {waiting.map((w) => (
        <MessageCard key={`${w.serverId}/${w.message.id}`} waiting={w} />
      ))}
    </View>
  );
}

function MessageCard({ waiting }: { waiting: Waiting }) {
  const { serverId, message } = waiting;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const act = (run: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    run().catch((e: unknown) => {
      setBusy(false);
      setError(String((e as Error).message ?? e));
    });
  };
  return (
    <View style={styles.card}>
      <Text style={styles.route}>
        <Text style={styles.party}>{partyLabel(message.from)}</Text>
        <Text style={styles.arrow}>{'  →  '}</Text>
        <Text style={styles.party}>{partyLabel(message.to)}</Text>
      </Text>
      <Text style={styles.when}>{sentAgo(message.sent_at)}</Text>
      <Text style={styles.body} numberOfLines={12} selectable>
        {message.body}
      </Text>
      {error ? <Text style={styles.error}>{error}</Text> : null}
      <View style={styles.actions}>
        <Action
          label="Approve"
          primary
          disabled={busy}
          onPress={() => act(() => approveMessage(serverId, message.id, false))}
        />
        <Action
          label="Always allow"
          hint="Approve, and let these two agents message each other from now on"
          disabled={busy}
          onPress={() => act(() => approveMessage(serverId, message.id, true))}
        />
        <View style={{ flex: 1 }} />
        <Action label="Deny" danger disabled={busy} onPress={() => act(() => denyMessage(serverId, message.id))} />
      </View>
    </View>
  );
}

function Action({
  label,
  hint,
  primary,
  danger,
  disabled,
  onPress,
}: {
  label: string;
  hint?: string;
  primary?: boolean;
  danger?: boolean;
  disabled?: boolean;
  onPress: () => void;
}) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityHint={hint}
      disabled={disabled}
      onPress={onPress}
      style={(state) => [
        styles.action,
        primary && styles.actionPrimary,
        (state as { hovered?: boolean }).hovered && !primary && styles.actionHover,
        disabled && { opacity: 0.5 },
      ]}
    >
      <Text style={[styles.actionText, primary && styles.actionPrimaryText, danger && { color: colors.danger }]}>
        {label}
      </Text>
    </Pressable>
  );
}

function sentAgo(sentAt: number): string {
  const seconds = Math.max(0, Math.floor(Date.now() / 1000 - sentAt));
  if (seconds < 60) return 'just now';
  if (seconds < 3600) return `${Math.floor(seconds / 60)} min ago`;
  return `${Math.floor(seconds / 3600)} h ago`;
}

const styles = StyleSheet.create({
  empty: { color: colors.dim, fontSize: 14, lineHeight: 20 },
  card: {
    backgroundColor: '#171a22',
    borderRadius: 12,
    borderWidth: 1,
    borderColor: '#2a2e3b',
    padding: 14,
    gap: 6,
  },
  route: { fontSize: 13.5 },
  party: { color: colors.text, fontWeight: '600' },
  arrow: { color: colors.dim },
  when: { color: '#8a90a2', fontSize: 12 },
  body: { color: '#d6d9e2', fontSize: 14, lineHeight: 20, marginTop: 4 },
  error: { color: colors.danger, fontSize: 12.5 },
  actions: { flexDirection: 'row', alignItems: 'center', gap: 8, marginTop: 8 },
  action: {
    paddingHorizontal: 12,
    paddingVertical: 7,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: '#2c303d',
  },
  actionHover: { backgroundColor: '#1d2029' },
  actionPrimary: { backgroundColor: colors.text, borderColor: colors.text },
  actionText: { color: colors.text, fontSize: 13.5, fontWeight: '600' },
  actionPrimaryText: { color: '#0b0d12' },
});
