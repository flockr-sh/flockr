import React, { useState } from 'react';
import { Alert, FlatList, Pressable, StyleSheet, Text, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { connections } from '../state/connections';
import { statusText, useAgents, useClientStatus, useServers, useTerminals } from '../state/hooks';
import type { Nav } from '../state/nav';
import { colors, statusColor, ui } from '../state/ui';
import { repairMessage, repairReason, repairShort } from '../state/storage';
import type { AgentRow, TerminalRow } from '../protocol';

interface Props {
  nav: Nav;
  serverId: string;
}

export function AgentsScreen({ nav, serverId }: Props) {
  const servers = useServers();
  const server = servers.find((s) => s.server_id === serverId);
  const repair = server ? repairReason(server) : null;
  const client = server && !repair ? connections.client(serverId) : null;
  const status = useClientStatus(client);
  const { rows, loaded, msg } = useAgents(client);
  const terminals = useTerminals(client, msg);
  const [tab, setTab] = useState<'agents' | 'terminals'>('agents');

  const remove = () =>
    Alert.alert('Remove server', `Unpair this phone from ${server?.name ?? 'this server'}?`, [
      { text: 'Cancel', style: 'cancel' },
      {
        text: 'Remove',
        style: 'destructive',
        onPress: () => {
          void connections.remove(serverId).then(() => nav.pop());
        },
      },
    ]);

  const open = (r: AgentRow) =>
    nav.push({
      name: 'terminal',
      serverId,
      paneId: r.agent.pane_id,
      title: `${r.name} · ${r.workspaceLabel} › ${r.tabLabel}`,
    });

  const openTerminal = (r: TerminalRow) =>
    nav.push({
      name: 'terminal',
      serverId,
      paneId: r.pane.pane_id,
      title: `${r.title} · ${r.workspaceLabel} › ${r.tabLabel}`,
    });

  return (
    <SafeAreaView style={ui.screen} edges={['top', 'left', 'right']}>
      <View style={ui.header}>
        <Pressable accessibilityRole="button" onPress={() => nav.pop()} style={ui.button}>
          <Text style={ui.buttonText}>‹</Text>
        </Pressable>
        <View style={{ flex: 1 }}>
          <Text style={ui.title} numberOfLines={1}>
            {server?.name ?? 'Server'}
          </Text>
          <Text style={ui.sub}>{repair ? repairShort(repair) : statusText(status)}</Text>
        </View>
        <Pressable accessibilityRole="button" onPress={remove} style={ui.button}>
          <Text style={[ui.buttonText, { color: colors.danger }]}>Remove</Text>
        </Pressable>
      </View>
      {repair ? (
        <View style={styles.empty}>
          <Text style={ui.text}>{repairMessage(repair)}</Text>
          <Pressable
            accessibilityRole="button"
            style={[ui.button, ui.primary, { marginTop: 12 }]}
            onPress={() => nav.push({ name: 'pair' })}
          >
            <Text style={[ui.buttonText, ui.primaryText]}>Pair again</Text>
          </Pressable>
        </View>
      ) : (
        <>
          <View style={styles.tabs}>
            <TabButton label={`Agents (${rows.length})`} active={tab === 'agents'} onPress={() => setTab('agents')} />
            <TabButton
              label={`Terminals (${terminals.rows.length})`}
              active={tab === 'terminals'}
              onPress={() => setTab('terminals')}
            />
          </View>
          {tab === 'agents' ? (
            <FlatList
              data={rows}
              keyExtractor={(r) => r.agent.pane_id}
              contentContainerStyle={{ padding: 8, gap: 8 }}
              ListEmptyComponent={
                <View style={styles.empty}>
                  <Text style={ui.sub}>{loaded ? 'No agents.' : 'Waiting for the agent list…'}</Text>
                </View>
              }
              renderItem={({ item }) => <AgentCard row={item} onPress={() => open(item)} />}
            />
          ) : (
            <FlatList
              data={terminals.rows}
              keyExtractor={(r) => r.pane.pane_id}
              contentContainerStyle={{ padding: 8, gap: 8 }}
              ListEmptyComponent={
                <View style={styles.empty}>
                  <Text style={ui.sub}>{terminals.loaded ? 'No shell panes.' : 'Loading panes…'}</Text>
                </View>
              }
              renderItem={({ item }) => <TerminalCard row={item} onPress={() => openTerminal(item)} />}
            />
          )}
        </>
      )}
    </SafeAreaView>
  );
}

function TabButton({ label, active, onPress }: { label: string; active: boolean; onPress: () => void }) {
  return (
    <Pressable
      accessibilityRole="tab"
      accessibilityState={{ selected: active }}
      onPress={onPress}
      style={[styles.tab, active && styles.tabActive]}
    >
      <Text style={[ui.buttonText, { fontSize: 14 }, !active && { color: colors.dim }]}>{label}</Text>
    </Pressable>
  );
}

function TerminalCard({ row, onPress }: { row: TerminalRow; onPress: () => void }) {
  return (
    <Pressable
      accessibilityRole="button"
      onPress={onPress}
      style={({ pressed }) => [styles.card, pressed && { backgroundColor: colors.panelHi }]}
    >
      <Text style={styles.prompt}>$</Text>
      <View style={{ flex: 1 }}>
        <Text style={styles.name} numberOfLines={1}>
          {row.title}
        </Text>
        <Text style={ui.sub} numberOfLines={1}>
          {row.workspaceLabel} › {row.tabLabel}
        </Text>
        {row.cwd ? (
          <Text style={[ui.sub, { marginTop: 2 }]} numberOfLines={1} ellipsizeMode="head">
            {row.cwd}
          </Text>
        ) : null}
      </View>
    </Pressable>
  );
}

function AgentCard({ row, onPress }: { row: AgentRow; onPress: () => void }) {
  const title = row.agent.terminal_title_stripped || row.agent.title;
  return (
    <Pressable
      accessibilityRole="button"
      onPress={onPress}
      style={({ pressed }) => [styles.card, pressed && { backgroundColor: colors.panelHi }]}
    >
      <View style={[styles.dot, { backgroundColor: statusColor[row.status] }]} />
      <View style={{ flex: 1 }}>
        <View style={styles.line}>
          <Text style={styles.name} numberOfLines={1}>
            {row.name}
          </Text>
          <Text style={[styles.status, { color: statusColor[row.status] }]}>{row.status}</Text>
        </View>
        <Text style={ui.sub} numberOfLines={1}>
          {row.workspaceLabel} › {row.tabLabel}
        </Text>
        {title ? (
          <Text style={[ui.sub, { marginTop: 2 }]} numberOfLines={1}>
            {title}
          </Text>
        ) : null}
      </View>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  empty: { padding: 24, alignItems: 'center' },
  tabs: { flexDirection: 'row', gap: 6, paddingHorizontal: 8, paddingTop: 8 },
  tab: { flex: 1, alignItems: 'center', paddingVertical: 8, borderRadius: 8, backgroundColor: colors.panel },
  tabActive: { backgroundColor: colors.panelHi },
  prompt: { color: colors.dim, fontSize: 16, fontWeight: '700', width: 10, textAlign: 'center' },
  card: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    padding: 12,
    borderRadius: 10,
    backgroundColor: colors.panel,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: colors.border,
  },
  dot: { width: 10, height: 10, borderRadius: 5 },
  line: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  name: { color: colors.text, fontSize: 16, fontWeight: '600', flexShrink: 1 },
  status: { fontSize: 12, fontWeight: '600', textTransform: 'uppercase' },
});
