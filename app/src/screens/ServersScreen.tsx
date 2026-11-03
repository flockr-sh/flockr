import React from 'react';
import { FlatList, Pressable, StyleSheet, Text, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { connections } from '../state/connections';
import { statusText, useClientStatus, useServers } from '../state/hooks';
import type { Nav } from '../state/nav';
import { useWaitingMessages } from '../state/messages';
import { isExpoGo } from '../state/push';
import { repairReason, repairShort, StoredServer } from '../state/storage';
import { colors, ui } from '../state/ui';

export function ServersScreen({ nav, pushNote }: { nav: Nav; pushNote: string | null }) {
  const servers = useServers();
  const waiting = useWaitingMessages();
  return (
    <SafeAreaView style={ui.screen}>
      <View style={ui.header}>
        <Text style={[ui.title, { flex: 1 }]}>flockr</Text>
        <Pressable accessibilityRole="button" style={ui.button} onPress={() => nav.push({ name: 'settings' })}>
          <Text style={ui.buttonText}>Settings</Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          style={[ui.button, ui.primary]}
          onPress={() => nav.push({ name: 'pair' })}
        >
          <Text style={[ui.buttonText, ui.primaryText]}>Pair</Text>
        </Pressable>
      </View>
      {waiting.length > 0 ? (
        <Pressable
          accessibilityRole="button"
          onPress={() => nav.push({ name: 'messages' })}
          style={({ pressed }) => [styles.waiting, pressed && { opacity: 0.8 }]}
        >
          <Text style={styles.waitingText}>
            {waiting.length === 1 ? '1 message waits' : `${waiting.length} messages wait`} for your approval
          </Text>
          <Text style={styles.waitingText}>›</Text>
        </Pressable>
      ) : null}
      <FlatList
        data={servers}
        keyExtractor={(s) => s.server_id}
        contentContainerStyle={{ padding: 8, gap: 8 }}
        ListEmptyComponent={
          <View style={styles.empty}>
            <Text style={ui.text}>No servers yet.</Text>
            <Text style={[ui.sub, { marginTop: 8, textAlign: 'center' }]}>
              On your computer run `flockr mobile pair`, then tap Pair and scan the QR code.
            </Text>
          </View>
        }
        renderItem={({ item }) => (
          <ServerCard server={item} onPress={() => nav.push({ name: 'agents', serverId: item.server_id })} />
        )}
      />
      <Text style={styles.footer}>{isExpoGo ? 'Expo Go: push notifications need a dev build.' : (pushNote ?? '')}</Text>
    </SafeAreaView>
  );
}

function ServerCard({ server, onPress }: { server: StoredServer; onPress: () => void }) {
  const repair = repairReason(server);
  const client = repair ? null : connections.client(server.server_id);
  const status = useClientStatus(client);
  const host = server.hosts[server.lastHost ?? 0] ?? server.hosts[0];
  const via = server.via ? connections.server(server.via) : undefined;
  const where = server.via ? `via ${via?.name ?? 'flock'}` : `${host}:${server.port}`;
  return (
    <Pressable
      accessibilityRole="button"
      onPress={onPress}
      style={({ pressed }) => [styles.card, pressed && { backgroundColor: colors.panelHi }]}
    >
      <Text style={styles.name}>{server.name}</Text>
      <Text style={ui.sub}>
        {where} · {repair ? repairShort(repair) : statusText(status)}
      </Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  empty: { padding: 24, alignItems: 'center' },
  waiting: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    margin: 8,
    marginBottom: 0,
    padding: 14,
    borderRadius: 10,
    backgroundColor: '#2a2414',
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: colors.warn,
  },
  waitingText: { color: colors.warn, fontSize: 15, fontWeight: '600' },
  card: {
    padding: 14,
    borderRadius: 10,
    backgroundColor: colors.panel,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: colors.border,
    gap: 4,
  },
  name: { color: colors.text, fontSize: 17, fontWeight: '600' },
  footer: { color: colors.dim, fontSize: 11, textAlign: 'center', padding: 8 },
});
