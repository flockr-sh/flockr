// Server settings the app can change, per machine. For now: when messages
// between agents wait for approval (`[flock] approve_messages`).
import React, { useEffect, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { connections } from '../state/connections';
import { useClientStatus, useServers } from '../state/hooks';
import {
  APPROVE_MODES,
  loadApproveMode,
  messagesClient,
  saveApproveMode,
  type ApproveMode,
} from '../state/messages';
import type { Nav } from '../state/nav';
import type { StoredServer } from '../state/storage';
import { colors, ui } from '../state/ui';

export function SettingsScreen({ nav }: { nav: Nav }) {
  return (
    <SafeAreaView style={ui.screen}>
      <View style={ui.header}>
        <Pressable accessibilityRole="button" style={ui.button} onPress={() => nav.pop()}>
          <Text style={ui.buttonText}>‹</Text>
        </Pressable>
        <Text style={[ui.title, { flex: 1 }]}>Settings</Text>
      </View>
      <ScrollView contentContainerStyle={{ padding: 12 }}>
        <SettingsList />
      </ScrollView>
    </SafeAreaView>
  );
}

export function SettingsList() {
  const servers = useServers();
  return (
    <View style={{ gap: 12 }}>
      <Text style={styles.section}>Approve agent messages</Text>
      <Text style={styles.lead}>
        When a message from one agent to another waits for you. Each machine decides for its own agents.
      </Text>
      {servers.length === 0 ? <Text style={styles.lead}>No machines paired.</Text> : null}
      {servers.map((server) => (
        <MachineSetting key={server.server_id} server={server} />
      ))}
    </View>
  );
}

function MachineSetting({ server }: { server: StoredServer }) {
  const status = useClientStatus(connections.client(server.server_id));
  const open = status.state === 'open';
  const supported = open && !!messagesClient(server.server_id);
  const [mode, setMode] = useState<ApproveMode | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!supported) return;
    let live = true;
    loadApproveMode(server.server_id)
      .then((m) => live && setMode(m))
      .catch((e: unknown) => live && setError(String((e as Error).message ?? e)));
    return () => {
      live = false;
    };
  }, [server.server_id, supported]);

  const choose = (next: ApproveMode) => {
    const previous = mode;
    setMode(next);
    setError(null);
    saveApproveMode(server.server_id, next).catch((e: unknown) => {
      setMode(previous);
      setError(String((e as Error).message ?? e));
    });
  };

  const note = !open ? 'Not connected.' : !supported ? 'Agent messages are off here: flock is off, or this flockr is too old.' : null;
  const help = APPROVE_MODES.find((m) => m.mode === mode)?.help;
  return (
    <View style={styles.card}>
      <Text style={styles.machine}>{server.name}</Text>
      {note ? (
        <Text style={styles.note}>{note}</Text>
      ) : (
        <>
          <View style={styles.segments}>
            {APPROVE_MODES.map((m) => (
              <Pressable
                key={m.mode}
                accessibilityRole="button"
                accessibilityState={{ selected: mode === m.mode }}
                onPress={() => choose(m.mode)}
                style={(state) => [
                  styles.segment,
                  mode === m.mode && styles.segmentOn,
                  (state as { hovered?: boolean }).hovered && mode !== m.mode && styles.segmentHover,
                ]}
              >
                <Text style={[styles.segmentText, mode === m.mode && styles.segmentTextOn]}>{m.label}</Text>
              </Pressable>
            ))}
          </View>
          {help ? <Text style={styles.note}>{help}</Text> : null}
        </>
      )}
      {error ? <Text style={styles.error}>{error}</Text> : null}
    </View>
  );
}

const styles = StyleSheet.create({
  section: { color: colors.text, fontSize: 15, fontWeight: '700' },
  lead: { color: colors.dim, fontSize: 13.5, lineHeight: 19 },
  card: {
    backgroundColor: '#14161d',
    borderRadius: 12,
    borderWidth: 1,
    borderColor: '#23262f',
    padding: 14,
    gap: 10,
  },
  machine: { color: colors.text, fontSize: 14.5, fontWeight: '600' },
  note: { color: '#8a90a2', fontSize: 13, lineHeight: 18 },
  error: { color: colors.danger, fontSize: 12.5 },
  segments: {
    flexDirection: 'row',
    alignSelf: 'flex-start',
    backgroundColor: '#0f1116',
    borderRadius: 9,
    borderWidth: 1,
    borderColor: '#262a35',
    padding: 3,
    gap: 2,
  },
  segment: { paddingHorizontal: 12, paddingVertical: 6, borderRadius: 7 },
  segmentHover: { backgroundColor: '#1b1e27' },
  segmentOn: { backgroundColor: '#2a2e3b' },
  segmentText: { color: '#8f95a7', fontSize: 13, fontWeight: '600' },
  segmentTextOn: { color: colors.text },
});
