// The wide web layout: machines and their agents in a sidebar, the chosen
// agent's terminal in the main panel, like a chat app with a conversation list.
import React, { useMemo, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { connections } from '../state/connections';
import { statusText, useAgents, useClientStatus, useServers, useTerminals } from '../state/hooks';
import type { Nav } from '../state/nav';
import { repairReason, repairShort, StoredServer } from '../state/storage';
import { colors, statusColor } from '../state/ui';
import { agentDetail, type AgentRow, type ConnStatus, type TerminalRow } from '../protocol';
import { PairScreen } from '../screens/PairScreen';
import { TerminalScreen } from '../screens/TerminalScreen';

interface Selection {
  serverId: string;
  paneId: string;
  title: string;
  crumb: string;
}

const SIDEBAR = {
  bg: '#0b0c10',
  border: '#1b1d25',
  hover: '#14161d',
  active: '#1c1f29',
  dim: '#6f7587',
};

export function DesktopShell() {
  const servers = useServers();
  const [selected, setSelected] = useState<Selection | null>(null);
  const [pairing, setPairing] = useState(false);
  const pairNav: Nav = useMemo(
    () => ({ push: () => setPairing(false), pop: () => setPairing(false), reset: () => setPairing(false) }),
    [],
  );
  const choose = (s: Selection) => {
    setPairing(false);
    setSelected(s);
  };

  let main: React.ReactNode;
  if (pairing) {
    main = (
      <View style={shell.pairWrap}>
        <View style={shell.pairCard}>
          <PairScreen nav={pairNav} />
        </View>
      </View>
    );
  } else if (selected && connections.server(selected.serverId)) {
    main = (
      <TerminalScreen
        key={`${selected.serverId}/${selected.paneId}`}
        nav={pairNav}
        serverId={selected.serverId}
        paneId={selected.paneId}
        title={selected.title}
        crumb={selected.crumb}
        variant="desktop"
      />
    );
  } else {
    main = <Overview servers={servers} onSelect={choose} onPair={() => setPairing(true)} />;
  }

  return (
    <View style={shell.root}>
      <View style={side.wrap}>
        <View style={side.top}>
          <Pressable accessibilityRole="button" onPress={() => setSelected(null)} style={side.brand}>
            <View style={side.logo}>
              <Text style={side.logoText}>f</Text>
            </View>
            <Text style={side.brandText}>flockr</Text>
          </Pressable>
          <Pressable
            accessibilityRole="button"
            accessibilityLabel="Pair a server"
            onPress={() => setPairing(true)}
            style={(state) => [side.iconButton, (state as { hovered?: boolean }).hovered && side.iconHover]}
          >
            <Text style={side.iconText}>＋</Text>
          </Pressable>
        </View>
        <ScrollView style={{ flex: 1 }} contentContainerStyle={side.list}>
          {servers.length === 0 ? <Text style={side.empty}>No servers yet. Pair one with ＋.</Text> : null}
          {servers.map((server) => (
            <MachineGroup key={server.server_id} server={server} selected={selected} onSelect={choose} />
          ))}
        </ScrollView>
        <Text style={side.footer}>
          {servers.length} {servers.length === 1 ? 'machine' : 'machines'}
        </Text>
      </View>
      <View style={{ flex: 1 }}>{main}</View>
    </View>
  );
}

function connColor(status: ConnStatus): string {
  if (status.state === 'open') return colors.ok;
  if (status.state === 'failed') return colors.danger;
  return colors.warn;
}

function machineLabel(server: StoredServer): string {
  const via = server.via ? connections.server(server.via) : undefined;
  return via ? `via ${via.name}` : (server.hosts[server.lastHost ?? 0] ?? server.hosts[0] ?? '');
}

function MachineGroup({
  server,
  selected,
  onSelect,
}: {
  server: StoredServer;
  selected: Selection | null;
  onSelect: (s: Selection) => void;
}) {
  const repair = repairReason(server);
  const client = repair ? null : connections.client(server.server_id);
  const status = useClientStatus(client);
  const { rows, loaded, msg } = useAgents(client);
  const terminals = useTerminals(client, msg);
  const [open, setOpen] = useState(true);
  const select = (paneId: string, title: string, workspace: string, tab: string) =>
    onSelect({ serverId: server.server_id, paneId, title, crumb: `${server.name} › ${workspace} › ${tab}` });
  const isSelected = (paneId: string) => selected?.serverId === server.server_id && selected.paneId === paneId;

  return (
    <View style={side.group}>
      <Pressable
        accessibilityRole="button"
        onPress={() => setOpen(!open)}
        style={(state) => [side.machine, (state as { hovered?: boolean }).hovered && side.rowHover]}
      >
        <Text style={[side.chevron, open && side.chevronOpen]}>›</Text>
        <View style={[side.connDot, { backgroundColor: repair ? colors.danger : connColor(status) }]} />
        <View style={{ flex: 1, minWidth: 0 }}>
          <Text style={side.machineName} numberOfLines={1}>
            {server.name}
          </Text>
          <Text style={side.machineSub} numberOfLines={1}>
            {repair ? repairShort(repair) : status.state === 'open' ? machineLabel(server) : statusText(status)}
          </Text>
        </View>
        {rows.length > 0 ? <Text style={side.count}>{rows.length}</Text> : null}
      </Pressable>
      {open && !repair ? (
        <View style={side.children}>
          {rows.map((row) => (
            <AgentItem
              key={row.agent.pane_id}
              row={row}
              active={isSelected(row.agent.pane_id)}
              onPress={() => select(row.agent.pane_id, row.name, row.workspaceLabel, row.tabLabel)}
            />
          ))}
          {loaded && rows.length === 0 ? <Text style={side.none}>No agents</Text> : null}
          {terminals.rows.length > 0 ? <Text style={side.sub}>Terminals</Text> : null}
          {terminals.rows.map((row) => (
            <TerminalItem
              key={row.pane.pane_id}
              row={row}
              active={isSelected(row.pane.pane_id)}
              onPress={() => select(row.pane.pane_id, row.title, row.workspaceLabel, row.tabLabel)}
            />
          ))}
        </View>
      ) : null}
    </View>
  );
}

function StatusDot({ status }: { status: AgentRow['status'] }) {
  const ring = status === 'idle' || status === 'unknown';
  return (
    <View
      style={[
        side.statusDot,
        ring ? { borderWidth: 1.5, borderColor: statusColor[status] } : { backgroundColor: statusColor[status] },
      ]}
    />
  );
}

function AgentItem({ row, active, onPress }: { row: AgentRow; active: boolean; onPress: () => void }) {
  const activity = row.agent.terminal_title_stripped || row.agent.title || agentDetail(row);
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${row.name}, ${row.status}`}
      onPress={onPress}
      style={(state) => [
        side.item,
        active ? side.itemActive : (state as { hovered?: boolean }).hovered && side.rowHover,
      ]}
    >
      <StatusDot status={row.status} />
      <View style={{ flex: 1, minWidth: 0 }}>
        <Text style={[side.itemName, active && { color: colors.text }]} numberOfLines={1}>
          {row.name}
        </Text>
        {activity ? (
          <Text style={side.itemSub} numberOfLines={1}>
            {activity}
          </Text>
        ) : null}
      </View>
    </Pressable>
  );
}

function TerminalItem({ row, active, onPress }: { row: TerminalRow; active: boolean; onPress: () => void }) {
  return (
    <Pressable
      accessibilityRole="button"
      onPress={onPress}
      style={(state) => [
        side.item,
        active ? side.itemActive : (state as { hovered?: boolean }).hovered && side.rowHover,
      ]}
    >
      <Text style={side.prompt}>›_</Text>
      <Text style={[side.itemName, side.terminalName, active && { color: colors.text }]} numberOfLines={1}>
        {row.title}
      </Text>
    </Pressable>
  );
}

function Overview({
  servers,
  onSelect,
  onPair,
}: {
  servers: StoredServer[];
  onSelect: (s: Selection) => void;
  onPair: () => void;
}) {
  if (servers.length === 0) {
    return (
      <View style={over.center}>
        <Text style={over.hero}>Welcome to flockr</Text>
        <Text style={over.lead}>Pair a flockr server to see its agents here, and every machine in its flock.</Text>
        <Pressable accessibilityRole="button" onPress={onPair} style={over.cta}>
          <Text style={over.ctaText}>Pair a server</Text>
        </Pressable>
      </View>
    );
  }
  return (
    <ScrollView contentContainerStyle={over.page}>
      <Text style={over.hero}>Your agents</Text>
      <Text style={over.lead}>Pick an agent to open its terminal. Blocked and finished ones come first.</Text>
      <View style={over.grid}>
        {servers.map((server) => (
          <MachineCard key={server.server_id} server={server} onSelect={onSelect} />
        ))}
      </View>
    </ScrollView>
  );
}

const ORDER: Record<AgentRow['status'], number> = { blocked: 0, done: 1, working: 2, idle: 3, unknown: 4 };

function MachineCard({ server, onSelect }: { server: StoredServer; onSelect: (s: Selection) => void }) {
  const repair = repairReason(server);
  const client = repair ? null : connections.client(server.server_id);
  const status = useClientStatus(client);
  const { rows } = useAgents(client);
  const sorted = [...rows].sort((a, b) => ORDER[a.status] - ORDER[b.status]);
  return (
    <View style={over.card}>
      <View style={over.cardHead}>
        <View style={[side.connDot, { backgroundColor: repair ? colors.danger : connColor(status) }]} />
        <Text style={over.cardTitle} numberOfLines={1}>
          {server.name}
        </Text>
        <Text style={over.cardMeta}>
          {repair ? repairShort(repair) : `${rows.length} ${rows.length === 1 ? 'agent' : 'agents'}`}
        </Text>
      </View>
      {sorted.length === 0 ? <Text style={over.cardEmpty}>{repair ? '' : 'No agents running.'}</Text> : null}
      {sorted.slice(0, 6).map((row) => (
        <Pressable
          key={row.agent.pane_id}
          accessibilityRole="button"
          onPress={() =>
            onSelect({
              serverId: server.server_id,
              paneId: row.agent.pane_id,
              title: row.name,
              crumb: `${server.name} › ${row.workspaceLabel} › ${row.tabLabel}`,
            })
          }
          style={(state) => [over.agent, (state as { hovered?: boolean }).hovered && over.agentHover]}
        >
          <StatusDot status={row.status} />
          <Text style={over.agentName} numberOfLines={1}>
            {row.name}
          </Text>
          <Text style={over.agentWhere} numberOfLines={1}>
            {row.agent.terminal_title_stripped || row.workspaceLabel}
          </Text>
          <Text style={[over.badge, { color: statusColor[row.status] }]}>{row.status}</Text>
        </Pressable>
      ))}
    </View>
  );
}

const shell = StyleSheet.create({
  root: { flex: 1, flexDirection: 'row', backgroundColor: colors.bg },
  pairWrap: { flex: 1, alignItems: 'center', justifyContent: 'center', padding: 32 },
  pairCard: {
    width: '100%',
    maxWidth: 560,
    height: 520,
    borderRadius: 16,
    overflow: 'hidden',
    borderWidth: 1,
    borderColor: '#23262f',
  },
});

const side = StyleSheet.create({
  wrap: { width: 280, backgroundColor: SIDEBAR.bg, borderRightWidth: 1, borderRightColor: SIDEBAR.border },
  top: { height: 60, flexDirection: 'row', alignItems: 'center', paddingHorizontal: 14, gap: 8 },
  brand: { flex: 1, flexDirection: 'row', alignItems: 'center', gap: 10 },
  logo: {
    width: 26,
    height: 26,
    borderRadius: 8,
    backgroundColor: colors.accent,
    alignItems: 'center',
    justifyContent: 'center',
  },
  logoText: { color: '#0b0d12', fontWeight: '800', fontSize: 15 },
  brandText: { color: colors.text, fontSize: 16, fontWeight: '700', letterSpacing: 0.2 },
  iconButton: { width: 32, height: 32, borderRadius: 8, alignItems: 'center', justifyContent: 'center' },
  iconHover: { backgroundColor: SIDEBAR.active },
  iconText: { color: '#aab0c0', fontSize: 18 },
  list: { paddingHorizontal: 8, paddingBottom: 16, gap: 6 },
  empty: { color: SIDEBAR.dim, fontSize: 13, padding: 12 },
  group: { gap: 2 },
  machine: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingHorizontal: 8,
    paddingVertical: 7,
    borderRadius: 8,
  },
  chevron: { color: SIDEBAR.dim, fontSize: 15, width: 10, textAlign: 'center', lineHeight: 15 },
  chevronOpen: { transform: [{ rotate: '90deg' }] },
  connDot: { width: 7, height: 7, borderRadius: 4 },
  machineName: { color: '#d6d9e2', fontSize: 13.5, fontWeight: '600' },
  machineSub: { color: SIDEBAR.dim, fontSize: 11.5, marginTop: 1 },
  count: {
    color: SIDEBAR.dim,
    fontSize: 11,
    paddingHorizontal: 6,
    paddingVertical: 1,
    borderRadius: 6,
    backgroundColor: '#161821',
  },
  children: { paddingLeft: 14, gap: 1 },
  item: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingHorizontal: 10,
    paddingVertical: 7,
    borderRadius: 8,
  },
  itemActive: { backgroundColor: SIDEBAR.active },
  rowHover: { backgroundColor: SIDEBAR.hover },
  statusDot: { width: 8, height: 8, borderRadius: 4 },
  itemName: { color: '#b9bdc9', fontSize: 13.5, fontWeight: '500' },
  itemSub: { color: SIDEBAR.dim, fontSize: 11.5, marginTop: 2 },
  none: { color: SIDEBAR.dim, fontSize: 12, paddingHorizontal: 10, paddingVertical: 6 },
  sub: {
    color: '#555b6c',
    fontSize: 10.5,
    fontWeight: '700',
    letterSpacing: 0.8,
    textTransform: 'uppercase',
    paddingHorizontal: 10,
    paddingTop: 10,
    paddingBottom: 4,
  },
  prompt: { color: '#555b6c', fontSize: 11, fontWeight: '700', width: 12 },
  terminalName: { flex: 1, fontSize: 13 },
  footer: { color: '#4b5060', fontSize: 11.5, padding: 14, borderTopWidth: 1, borderTopColor: SIDEBAR.border },
});

const over = StyleSheet.create({
  center: { flex: 1, alignItems: 'center', justifyContent: 'center', padding: 40 },
  page: { padding: 48, maxWidth: 1100, width: '100%', alignSelf: 'center' },
  hero: { color: colors.text, fontSize: 28, fontWeight: '700', letterSpacing: -0.4 },
  lead: { color: '#8a90a2', fontSize: 15, marginTop: 8, marginBottom: 28, textAlign: 'left' },
  cta: { marginTop: 4, backgroundColor: colors.text, borderRadius: 10, paddingHorizontal: 18, paddingVertical: 10 },
  ctaText: { color: '#0b0d12', fontWeight: '700', fontSize: 14 },
  grid: { flexDirection: 'row', flexWrap: 'wrap', gap: 16 },
  card: {
    flexGrow: 1,
    flexBasis: 420,
    backgroundColor: '#14161d',
    borderRadius: 14,
    borderWidth: 1,
    borderColor: '#20232d',
    padding: 16,
    gap: 4,
  },
  cardHead: { flexDirection: 'row', alignItems: 'center', gap: 8, marginBottom: 8 },
  cardTitle: { color: colors.text, fontSize: 15, fontWeight: '650' as never, flex: 1 },
  cardMeta: { color: '#6f7587', fontSize: 12 },
  cardEmpty: { color: '#6f7587', fontSize: 13, paddingVertical: 6 },
  agent: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingHorizontal: 8,
    paddingVertical: 8,
    borderRadius: 8,
  },
  agentHover: { backgroundColor: '#1b1e27' },
  agentName: { color: '#d6d9e2', fontSize: 13.5, fontWeight: '600', width: 120 },
  agentWhere: { color: '#6f7587', fontSize: 12.5, flex: 1 },
  badge: { fontSize: 11.5, fontWeight: '600', textTransform: 'capitalize' },
});
