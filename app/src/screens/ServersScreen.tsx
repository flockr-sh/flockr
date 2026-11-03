// Home: every agent on every machine, the ones that need you first.
import React, { useCallback, useState } from 'react';
import { Image, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { connections } from '../state/connections';
import { statusText, useAgents, useClientStatus, useServers } from '../state/hooks';
import type { Nav } from '../state/nav';
import { useWaitingMessages } from '../state/messages';
import { isExpoGo } from '../state/push';
import { repairReason, repairShort, StoredServer } from '../state/storage';
import { colors, statusColor, ui } from '../state/ui';
import { byAttention, type AgentRow, type AgentStatus } from '../protocol';
import { AgentCard } from './AgentsScreen';
import { DemoTag } from './DemoTag';
import { NeedsYouCard } from './NeedsYouCard';

type Counts = Partial<Record<AgentStatus, number>>;

const SUMMARY: AgentStatus[] = ['blocked', 'done', 'working', 'idle'];

export function ServersScreen({ nav, pushNote }: { nav: Nav; pushNote: string | null }) {
  const servers = useServers();
  const waiting = useWaitingMessages();
  const [counts, setCounts] = useState<Record<string, Counts>>({});
  const report = useCallback((serverId: string, next: Counts) => {
    setCounts((prev) => (sameCounts(prev[serverId], next) ? prev : { ...prev, [serverId]: next }));
  }, []);
  const totals: Counts = {};
  for (const server of servers) {
    for (const [status, n] of Object.entries(counts[server.server_id] ?? {})) {
      totals[status as AgentStatus] = (totals[status as AgentStatus] ?? 0) + (n ?? 0);
    }
  }
  const note = (isExpoGo && __DEV__ ? 'Expo Go: push notifications need a dev build.' : null) ?? pushNote;

  return (
    <SafeAreaView style={ui.screen} edges={['top', 'left', 'right']}>
      <View style={ui.header}>
        <Image source={require('../../assets/brand/mark-tide-96.png')} style={styles.mark} />
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
      <ScrollView contentContainerStyle={styles.page}>
        {servers.length === 0 ? (
          <View style={styles.empty}>
            <Text style={ui.text}>No servers yet.</Text>
            <Text style={[ui.sub, { marginTop: 8, textAlign: 'center' }]}>
              On your computer run `flockr mobile pair`, then tap Pair and scan the QR code.
            </Text>
            <Pressable
              accessibilityRole="button"
              style={[ui.button, { marginTop: 20 }]}
              onPress={() => void connections.startDemo()}
            >
              <Text style={ui.buttonText}>Try the demo</Text>
            </Pressable>
            <Text style={[ui.sub, { marginTop: 8, textAlign: 'center' }]}>
              Two made-up machines with agents, so you can look around without a server.
            </Text>
          </View>
        ) : (
          <View style={styles.summary}>
            {SUMMARY.filter((s) => totals[s]).map((s) => (
              <View key={s} style={styles.chip}>
                <View style={[styles.chipDot, { backgroundColor: statusColor[s] }]} />
                <Text style={styles.chipText}>
                  {totals[s]} {s}
                </Text>
              </View>
            ))}
          </View>
        )}
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
        {servers.map((server) => (
          <MachineSection key={server.server_id} server={server} nav={nav} onCounts={report} />
        ))}
        {note ? <Text style={styles.footer}>{note}</Text> : null}
      </ScrollView>
    </SafeAreaView>
  );
}

function MachineSection({
  server,
  nav,
  onCounts,
}: {
  server: StoredServer;
  nav: Nav;
  onCounts: (serverId: string, counts: Counts) => void;
}) {
  const repair = repairReason(server);
  const client = repair ? null : connections.client(server.server_id);
  const status = useClientStatus(client);
  const { rows, loaded } = useAgents(client);
  const sorted = byAttention(rows);
  const counts: Counts = {};
  for (const row of rows) counts[row.status] = (counts[row.status] ?? 0) + 1;
  React.useEffect(() => onCounts(server.server_id, counts));

  const via = server.via ? connections.server(server.via) : undefined;
  const state = repair ? repairShort(repair) : statusText(status);
  const meta = [via ? `via ${via.name}` : null, state].filter(Boolean).join(' · ');
  const connected = !repair && status.state === 'open';

  const open = (r: AgentRow) =>
    nav.push({
      name: 'terminal',
      serverId: server.server_id,
      paneId: r.agent.pane_id,
      title: `${r.name} · ${r.workspaceLabel} › ${r.tabLabel}`,
    });

  return (
    <View style={styles.section}>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`${server.name}, ${state}`}
        onPress={() => nav.push({ name: 'agents', serverId: server.server_id })}
        style={({ pressed }) => [styles.machine, pressed && { opacity: 0.7 }]}
      >
        <View style={[styles.connDot, { backgroundColor: connected ? colors.ok : repair ? colors.danger : colors.warn }]} />
        <Text style={styles.machineName} numberOfLines={1}>
          {server.name}
        </Text>
        <DemoTag show={!!server.demo} />
        <Text style={styles.machineMeta} numberOfLines={1}>
          {meta}
        </Text>
        <Text style={styles.chevron}>›</Text>
      </Pressable>
      {sorted.map((row) => {
        const card = (
          <AgentCard
            key={row.agent.pane_id}
            row={row}
            onPress={() => open(row)}
            detail={[row.workspaceLabel, row.branch].filter(Boolean).join(' · ')}
            note={row.status === 'blocked' ? 'Open to answer' : undefined}
          />
        );
        return row.status === 'blocked' ? (
          <NeedsYouCard
            key={row.agent.pane_id}
            row={row}
            client={client}
            serverId={server.server_id}
            where={[server.name, row.workspaceLabel].filter(Boolean).join(' · ')}
            onOpen={() => open(row)}
            fallback={card}
          />
        ) : (
          card
        );
      })}
      {connected && loaded && rows.length === 0 ? <Text style={styles.none}>No agents running.</Text> : null}
    </View>
  );
}

function sameCounts(a: Counts | undefined, b: Counts): boolean {
  if (!a) return false;
  const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
  for (const k of keys) if (a[k as AgentStatus] !== b[k as AgentStatus]) return false;
  return true;
}

const styles = StyleSheet.create({
  mark: { width: 22, height: 22 },
  page: { padding: 12, gap: 8, paddingBottom: 32 },
  empty: { padding: 24, alignItems: 'center' },
  summary: { flexDirection: 'row', flexWrap: 'wrap', gap: 8, marginBottom: 4 },
  chip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 7,
    paddingHorizontal: 11,
    paddingVertical: 6,
    borderRadius: 999,
    backgroundColor: colors.panel,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: colors.border,
  },
  chipDot: { width: 8, height: 8, borderRadius: 4 },
  chipText: { color: colors.text, fontSize: 13.5, fontWeight: '600' },
  waiting: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    padding: 14,
    borderRadius: 10,
    backgroundColor: '#2a2414',
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: colors.warn,
  },
  waitingText: { color: colors.warn, fontSize: 15, fontWeight: '600' },
  section: { gap: 8, marginTop: 8 },
  machine: { flexDirection: 'row', alignItems: 'center', gap: 8, paddingHorizontal: 4, paddingVertical: 4 },
  connDot: { width: 7, height: 7, borderRadius: 4 },
  machineName: { color: colors.text, fontSize: 16, fontWeight: '700', flexShrink: 1 },
  machineMeta: { color: colors.dim, fontSize: 13, flex: 1 },
  chevron: { color: colors.dim, fontSize: 20 },
  none: { color: colors.dim, fontSize: 13.5, paddingHorizontal: 4 },
  footer: { color: colors.dim, fontSize: 11, textAlign: 'center', padding: 8 },
});
