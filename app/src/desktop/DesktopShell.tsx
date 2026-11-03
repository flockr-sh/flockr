// The wide web layout: machines and their agents in a sidebar, the chosen
// agent's terminal in the main panel, like a chat app with a conversation list.
import React, { useEffect, useMemo, useRef, useState } from 'react';
import { Image, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { connections, InbandNotify } from '../state/connections';
import { statusText, useAgents, useClientStatus, useServers, useTerminals } from '../state/hooks';
import type { Nav } from '../state/nav';
import { repairReason, repairShort, StoredServer } from '../state/storage';
import { colors, statusColor } from '../state/ui';
import {
  agentDetail,
  normalizeStatus,
  type AgentRow,
  type ConnStatus,
  type TerminalRow,
  type WorkspaceInfo,
} from '../protocol';
import { MessagesList } from '../screens/MessagesScreen';
import { PairScreen } from '../screens/PairScreen';
import { SettingsList } from '../screens/SettingsScreen';
import { TerminalScreen } from '../screens/TerminalScreen';
import { useWaitingMessages } from '../state/messages';

interface Selection {
  serverId: string;
  paneId: string;
  title: string;
  crumb: string;
}

const SIDEBAR = {
  bg: '#0b0c10',
  border: '#262a35',
  hover: '#171920',
  active: '#1f2330',
  dim: '#8a90a2',
};

export function DesktopShell() {
  const servers = useServers();
  const [selected, setSelected] = useState<Selection | null>(null);
  const [pairing, setPairing] = useState(false);
  // A page in the main panel instead of a terminal or the overview.
  const [page, setPage] = useState<'messages' | 'settings' | null>(null);
  const waiting = useWaitingMessages();
  const pairNav: Nav = useMemo(
    () => ({ push: () => setPairing(false), pop: () => setPairing(false), reset: () => setPairing(false) }),
    [],
  );
  const choose = (s: Selection) => {
    setPairing(false);
    setPage(null);
    setSelected(s);
  };
  const openPage = (next: 'messages' | 'settings') => {
    setPairing(false);
    setPage(next);
  };
  const toasts = useToasts();
  const [menu, setMenu] = useState<MenuState | null>(null);

  let main: React.ReactNode;
  if (pairing) {
    main = (
      <View style={shell.pairWrap}>
        <View style={shell.pairCard}>
          <PairScreen nav={pairNav} />
        </View>
      </View>
    );
  } else if (page) {
    main = (
      <ScrollView contentContainerStyle={over.page}>
        <Text style={over.hero}>{page === 'messages' ? 'Messages' : 'Settings'}</Text>
        <Text style={over.lead}>
          {page === 'messages'
            ? 'Messages between agents that wait for you, from every machine.'
            : 'Settings for each paired machine.'}
        </Text>
        <View style={{ maxWidth: 760 }}>{page === 'messages' ? <MessagesList /> : <SettingsList />}</View>
      </ScrollView>
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
    main = (
      <Overview
        servers={servers}
        waiting={waiting.length}
        onSelect={choose}
        onPair={() => setPairing(true)}
        onMessages={() => openPage('messages')}
      />
    );
  }

  return (
    <View style={shell.root}>
      <View style={side.wrap}>
        <View style={side.top}>
          <Pressable
            accessibilityRole="button"
            onPress={() => {
              setSelected(null);
              setPage(null);
              setPairing(false);
            }}
            style={side.brand}
          >
            <Image source={require('../../assets/brand/mark-tide-96.png')} style={side.logo} accessibilityIgnoresInvertColors />
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
        <View style={side.nav}>
          <NavItem
            label="Messages"
            count={waiting.length}
            active={page === 'messages'}
            onPress={() => openPage('messages')}
          />
        </View>
        <ScrollView style={{ flex: 1 }} contentContainerStyle={side.list}>
          {servers.length === 0 ? <Text style={side.empty}>No servers yet. Pair one with ＋.</Text> : null}
          {servers.map((server) => (
            <MachineGroup
              key={server.server_id}
              server={server}
              selected={selected}
              onSelect={choose}
              onMenu={(x, y) =>
                setMenu({
                  x,
                  y,
                  items: [
                    {
                      label: `Remove ${server.name}`,
                      danger: true,
                      onPress: () => {
                        if (
                          selected?.serverId === server.server_id ||
                          selected?.serverId.startsWith(`${server.server_id}~`)
                        )
                          setSelected(null);
                        void connections.remove(server.server_id);
                      },
                    },
                  ],
                })
              }
            />
          ))}
        </ScrollView>
        <View style={side.footer}>
          <Text style={side.footerText}>
            {servers.length} {servers.length === 1 ? 'machine' : 'machines'}
          </Text>
          <Pressable
            accessibilityRole="button"
            onPress={() => openPage('settings')}
            style={(state) => [
              side.footerButton,
              ((state as { hovered?: boolean }).hovered || page === 'settings') && side.iconHover,
            ]}
          >
            <Text style={side.footerButtonText}>Settings</Text>
          </Pressable>
        </View>
      </View>
      <View style={{ flex: 1 }}>{main}</View>
      {menu ? <ContextMenu menu={menu} onClose={() => setMenu(null)} /> : null}
      <View style={toast.stack} pointerEvents="box-none">
        {toasts.list.map((n) => (
          <Toast
            key={n.key}
            notify={n}
            onClose={() => toasts.dismiss(n.key)}
            onOpen={() => {
              toasts.dismiss(n.key);
              choose({ serverId: n.server_id, paneId: n.pane_id, title: n.title, crumb: n.body });
            }}
          />
        ))}
      </View>
    </View>
  );
}

function NavItem({
  label,
  count,
  active,
  onPress,
}: {
  label: string;
  count: number;
  active: boolean;
  onPress: () => void;
}) {
  return (
    <Pressable
      accessibilityRole="button"
      onPress={onPress}
      style={(state) => [
        side.navItem,
        (state as { hovered?: boolean }).hovered && { backgroundColor: SIDEBAR.hover },
        active && { backgroundColor: SIDEBAR.active },
      ]}
    >
      <Text style={side.navText}>{label}</Text>
      {count > 0 ? <Text style={side.navBadge}>{count}</Text> : null}
    </Pressable>
  );
}

interface MenuItem {
  label: string;
  danger?: boolean;
  onPress: () => void;
}

interface MenuState {
  x: number;
  y: number;
  items: MenuItem[];
}

/** Calls `onMenu` with the pointer position on a right click, instead of the browser's menu. */
function useContextMenu(onMenu: (x: number, y: number) => void) {
  const ref = useRef<unknown>(null);
  const handler = useRef(onMenu);
  handler.current = onMenu;
  useEffect(() => {
    const element = ref.current as HTMLElement | null;
    if (!element?.addEventListener) return;
    const listener = (e: MouseEvent) => {
      e.preventDefault();
      handler.current(e.clientX, e.clientY);
    };
    element.addEventListener('contextmenu', listener);
    return () => element.removeEventListener('contextmenu', listener);
  }, []);
  return ref;
}

function ContextMenu({ menu, onClose }: { menu: MenuState; onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);
  return (
    <Pressable style={StyleSheet.absoluteFill} onPress={onClose}>
      <View style={[menuStyle.card, { left: menu.x, top: menu.y }]}>
        {menu.items.map((item) => (
          <Pressable
            key={item.label}
            accessibilityRole="menuitem"
            onPress={() => {
              onClose();
              item.onPress();
            }}
            style={(state) => [menuStyle.item, (state as { hovered?: boolean }).hovered && menuStyle.itemHover]}
          >
            <Text style={[menuStyle.label, item.danger && { color: colors.danger }]}>{item.label}</Text>
          </Pressable>
        ))}
      </View>
    </Pressable>
  );
}

const menuStyle = StyleSheet.create({
  card: {
    position: 'absolute',
    minWidth: 190,
    padding: 4,
    borderRadius: 8,
    backgroundColor: '#1b1d24',
    borderWidth: 1,
    borderColor: '#2a2d37',
    shadowColor: '#000',
    shadowOpacity: 0.45,
    shadowRadius: 16,
    shadowOffset: { width: 0, height: 6 },
  },
  item: { paddingHorizontal: 10, paddingVertical: 7, borderRadius: 5 },
  itemHover: { backgroundColor: '#262933' },
  label: { color: colors.text, fontSize: 13 },
});

type KeyedNotify = InbandNotify & { key: number };

const TOAST_MS = 7000;
const MAX_TOASTS = 3;

/** In-band alerts as toasts in the corner; each closes itself after a while. */
function useToasts() {
  const [list, setList] = useState<KeyedNotify[]>([]);
  const next = useRef(0);
  const timers = useRef(new Map<number, ReturnType<typeof setTimeout>>());
  const dismiss = (key: number) => {
    clearTimeout(timers.current.get(key));
    timers.current.delete(key);
    setList((l) => l.filter((n) => n.key !== key));
  };
  useEffect(
    () =>
      connections.onNotify((n) => {
        const key = next.current++;
        setList((l) => [...l, { ...n, key }].slice(-MAX_TOASTS));
        timers.current.set(
          key,
          setTimeout(() => dismiss(key), TOAST_MS),
        );
      }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  return { list, dismiss };
}

function Toast({ notify, onOpen, onClose }: { notify: KeyedNotify; onOpen: () => void; onClose: () => void }) {
  const blocked = notify.kind === 'blocked';
  return (
    <Pressable
      accessibilityRole="button"
      onPress={onOpen}
      style={(state) => [toast.card, (state as { hovered?: boolean }).hovered && toast.cardHover]}
    >
      <View style={[toast.dot, { backgroundColor: blocked ? statusColor.blocked : statusColor.done }]} />
      <View style={{ flex: 1, minWidth: 0 }}>
        <Text style={toast.title} numberOfLines={1}>
          {notify.title}
        </Text>
        <Text style={toast.body} numberOfLines={1}>
          {notify.body}
        </Text>
      </View>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel="Dismiss"
        onPress={onClose}
        style={(state) => [toast.close, (state as { hovered?: boolean }).hovered && toast.closeHover]}
      >
        <Text style={toast.closeText}>×</Text>
      </Pressable>
    </Pressable>
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
  onMenu,
}: {
  server: StoredServer;
  selected: Selection | null;
  onSelect: (s: Selection) => void;
  onMenu: (x: number, y: number) => void;
}) {
  const row = useContextMenu(onMenu);
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
        ref={row as never}
        accessibilityRole="button"
        onPress={() => setOpen(!open)}
        style={(state) => [side.machine, (state as { hovered?: boolean }).hovered && side.rowHover]}
      >
        {(state) => (
          <>
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
            {(state as { hovered?: boolean }).hovered ? (
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={`More for ${server.name}`}
                onPress={(e) => {
                  const ev = e.nativeEvent as unknown as { pageX: number; pageY: number };
                  onMenu(ev.pageX, ev.pageY);
                }}
                style={side.more}
              >
                <Text style={side.moreText}>⋯</Text>
              </Pressable>
            ) : rows.length > 0 ? (
              <Text style={side.count}>{rows.length}</Text>
            ) : null}
          </>
        )}
      </Pressable>
      {open && !repair ? (
        <View style={side.children}>
          {(msg?.workspaces ?? [])
            .slice()
            .sort((a, b) => a.number - b.number)
            .map((ws) => (
              <WorkspaceGroup
                key={ws.workspace_id}
                workspace={ws}
                agents={rows.filter((r) => r.agent.workspace_id === ws.workspace_id)}
                terminals={terminals.rows.filter((t) => t.pane.workspace_id === ws.workspace_id)}
                isSelected={isSelected}
                onSelect={select}
              />
            ))}
          {loaded && (msg?.workspaces ?? []).length === 0 ? <Text style={side.none}>No workspaces</Text> : null}
        </View>
      ) : null}
    </View>
  );
}

function WorkspaceGroup({
  workspace,
  agents,
  terminals,
  isSelected,
  onSelect,
}: {
  workspace: WorkspaceInfo;
  agents: AgentRow[];
  terminals: TerminalRow[];
  isSelected: (paneId: string) => boolean;
  onSelect: (paneId: string, title: string, workspace: string, tab: string) => void;
}) {
  const [open, setOpen] = useState(agents.length > 0);
  const [shellsOpen, setShellsOpen] = useState(false);
  const branch = agents.find((a) => a.branch)?.branch;
  const hasSelected =
    agents.some((a) => isSelected(a.agent.pane_id)) || terminals.some((t) => isSelected(t.pane.pane_id));
  const status = normalizeStatus(workspace.agent_status);
  return (
    <View>
      <Pressable
        accessibilityRole="button"
        onPress={() => setOpen(!open)}
        style={(state) => [
          side.workspace,
          !open && hasSelected && side.itemActive,
          (state as { hovered?: boolean }).hovered && side.rowHover,
        ]}
      >
        <StatusDot status={status} />
        <View style={{ flex: 1, minWidth: 0 }}>
          <Text style={side.workspaceName} numberOfLines={1}>
            <Text style={side.workspaceNumber}>{workspace.number} </Text>
            {workspace.label}
          </Text>
          {branch ? (
            <Text style={side.branch} numberOfLines={1}>
              {branch}
            </Text>
          ) : null}
        </View>
        {!open && agents.length + terminals.length > 0 ? (
          <Text style={side.count}>{agents.length + terminals.length}</Text>
        ) : null}
      </Pressable>
      {open ? (
        <View style={side.wsChildren}>
          {agents.map((row) => (
            <AgentItem
              key={row.agent.pane_id}
              row={row}
              active={isSelected(row.agent.pane_id)}
              onPress={() => onSelect(row.agent.pane_id, row.name, row.workspaceLabel, row.tabLabel)}
            />
          ))}
          {terminals.length > 0 ? (
            <Pressable
              accessibilityRole="button"
              onPress={() => setShellsOpen(!shellsOpen)}
              style={(state) => [side.shellToggle, (state as { hovered?: boolean }).hovered && side.rowHover]}
            >
              <Text style={side.prompt}>›_</Text>
              <Text style={side.shellToggleText}>
                {terminals.length} {terminals.length === 1 ? 'terminal' : 'terminals'}
              </Text>
              <Text style={[side.chevron, shellsOpen && side.chevronOpen]}>›</Text>
            </Pressable>
          ) : null}
          {shellsOpen || terminals.some((t) => isSelected(t.pane.pane_id))
            ? terminals.map((row) => (
                <TerminalItem
                  key={row.pane.pane_id}
                  row={row}
                  active={isSelected(row.pane.pane_id)}
                  onPress={() => onSelect(row.pane.pane_id, shellLabel(row), row.workspaceLabel, row.tabLabel)}
                />
              ))
            : null}
        </View>
      ) : null}
    </View>
  );
}

/** A shell by its folder (`~/Projects/x`), unless it set a title of its own. */
function shellLabel(row: TerminalRow): string {
  const title = row.title.trim();
  const cwd = (row.pane.foreground_cwd || row.cwd || '').replace(/^\/home\/[^/]+/, '~');
  if (title && title.toLowerCase() !== 'shell' && !title.includes('@')) return title;
  return cwd || title || 'Shell';
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
        <Text style={[side.itemName, active && { color: colors.accent }]} numberOfLines={1}>
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
        {shellLabel(row)}
      </Text>
    </Pressable>
  );
}

function Overview({
  servers,
  waiting,
  onSelect,
  onPair,
  onMessages,
}: {
  servers: StoredServer[];
  waiting: number;
  onSelect: (s: Selection) => void;
  onPair: () => void;
  onMessages: () => void;
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
      <Text style={over.lead}>
        Pick an agent to open its terminal. Each machine lists agents that need you first (blocked, then finished), then
        working, then idle.
      </Text>
      {waiting > 0 ? (
        <Pressable
          accessibilityRole="button"
          onPress={onMessages}
          style={(state) => [over.waiting, (state as { hovered?: boolean }).hovered && { borderColor: colors.warn }]}
        >
          <Text style={over.waitingText}>
            {waiting === 1 ? '1 message waits' : `${waiting} messages wait`} for your approval
          </Text>
          <Text style={over.waitingLink}>Review</Text>
        </Pressable>
      ) : null}
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
      {sorted.map((row) => (
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
            {row.workspaceLabel}
          </Text>
          <Text style={over.agentWhere} numberOfLines={1}>
            {row.agent.terminal_title_stripped || row.name}
          </Text>
          <Text style={[over.badge, { color: statusColor[row.status] }]}>{row.status}</Text>
        </Pressable>
      ))}
    </View>
  );
}

const toast = StyleSheet.create({
  stack: { position: 'absolute', right: 20, bottom: 20, gap: 8, alignItems: 'flex-end' },
  card: {
    width: 340,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    paddingLeft: 14,
    paddingRight: 8,
    paddingVertical: 11,
    borderRadius: 10,
    backgroundColor: '#1b1d24',
    borderWidth: 1,
    borderColor: '#2a2d37',
    shadowColor: '#000',
    shadowOpacity: 0.45,
    shadowRadius: 20,
    shadowOffset: { width: 0, height: 8 },
  },
  cardHover: { backgroundColor: '#20232b' },
  dot: { width: 8, height: 8, borderRadius: 4 },
  title: { color: colors.text, fontSize: 13.5, fontWeight: '600' },
  body: { color: '#8a90a2', fontSize: 12.5, marginTop: 2 },
  close: { width: 26, height: 26, borderRadius: 6, alignItems: 'center', justifyContent: 'center' },
  closeHover: { backgroundColor: '#2a2d37' },
  closeText: { color: '#8a90a2', fontSize: 17, lineHeight: 18 },
});

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
    borderColor: '#2a2e3b',
  },
});

const side = StyleSheet.create({
  wrap: { width: 280, backgroundColor: SIDEBAR.bg, borderRightWidth: 1, borderRightColor: SIDEBAR.border },
  top: { height: 60, flexDirection: 'row', alignItems: 'center', paddingHorizontal: 14, gap: 8 },
  brand: { flex: 1, flexDirection: 'row', alignItems: 'center', gap: 10 },
  logo: { width: 22, height: 22 },
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
  more: { width: 24, height: 22, borderRadius: 6, alignItems: 'center', justifyContent: 'center' },
  moreText: { color: '#aab0c0', fontSize: 15, lineHeight: 16 },
  count: {
    color: SIDEBAR.dim,
    fontSize: 11,
    paddingHorizontal: 6,
    paddingVertical: 1,
    borderRadius: 6,
    backgroundColor: '#161821',
  },
  children: { paddingLeft: 10, gap: 1 },
  workspace: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingHorizontal: 10,
    paddingVertical: 6,
    borderRadius: 8,
  },
  workspaceName: { color: '#c9cdd8', fontSize: 13.5, fontWeight: '600' },
  workspaceNumber: { color: '#8a90a2', fontWeight: '600' },
  branch: { color: '#8a90a2', fontSize: 11.5, marginTop: 1 },
  wsChildren: { paddingLeft: 16, gap: 1, marginBottom: 4 },
  shellToggle: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingHorizontal: 10,
    paddingVertical: 5,
    borderRadius: 8,
  },
  shellToggleText: { flex: 1, color: '#8a90a2', fontSize: 12.5 },
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
    color: '#8a90a2',
    fontSize: 10.5,
    fontWeight: '700',
    letterSpacing: 0.8,
    textTransform: 'uppercase',
    paddingHorizontal: 10,
    paddingTop: 10,
    paddingBottom: 4,
  },
  prompt: { color: '#8a90a2', fontSize: 11, fontWeight: '700', width: 12 },
  terminalName: { flex: 1, fontSize: 13 },
  footer: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 14,
    paddingVertical: 8,
    borderTopWidth: 1,
    borderTopColor: SIDEBAR.border,
  },
  footerText: { color: '#8a90a2', fontSize: 11.5, flex: 1 },
  footerButton: { paddingHorizontal: 10, paddingVertical: 6, borderRadius: 7 },
  footerButtonText: { color: '#aab0c0', fontSize: 12.5, fontWeight: '600' },
  nav: { paddingHorizontal: 8, paddingBottom: 6 },
  navItem: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 10,
    paddingVertical: 7,
    borderRadius: 8,
  },
  navText: { color: '#d6d9e2', fontSize: 13.5, fontWeight: '600', flex: 1 },
  navBadge: {
    color: '#0b0d12',
    backgroundColor: colors.warn,
    fontSize: 11,
    fontWeight: '700',
    paddingHorizontal: 7,
    paddingVertical: 1,
    borderRadius: 8,
    overflow: 'hidden',
  },
});

const over = StyleSheet.create({
  center: { flex: 1, alignItems: 'center', justifyContent: 'center', padding: 40 },
  page: { padding: 48, maxWidth: 1100, width: '100%', alignSelf: 'center' },
  hero: { color: colors.text, fontSize: 28, fontWeight: '700', letterSpacing: -0.4 },
  lead: { color: '#8a90a2', fontSize: 15, marginTop: 8, marginBottom: 28, textAlign: 'left' },
  cta: { marginTop: 4, backgroundColor: colors.text, borderRadius: 10, paddingHorizontal: 18, paddingVertical: 10 },
  ctaText: { color: '#0b0d12', fontWeight: '700', fontSize: 14 },
  grid: { flexDirection: 'row', flexWrap: 'wrap', gap: 16 },
  waiting: {
    flexDirection: 'row',
    alignItems: 'center',
    marginBottom: 20,
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: '#4a3c22',
    backgroundColor: '#1a1610',
  },
  waitingText: { color: colors.warn, fontSize: 14, fontWeight: '600', flex: 1 },
  waitingLink: { color: colors.text, fontSize: 13.5, fontWeight: '600' },
  card: {
    flexGrow: 1,
    flexBasis: 420,
    backgroundColor: '#171a22',
    borderRadius: 14,
    borderWidth: 1,
    borderColor: '#2a2e3b',
    padding: 16,
    gap: 4,
  },
  cardHead: { flexDirection: 'row', alignItems: 'center', gap: 8, marginBottom: 8 },
  cardTitle: { color: colors.text, fontSize: 15, fontWeight: '650' as never, flex: 1 },
  cardMeta: { color: '#8a90a2', fontSize: 12 },
  cardEmpty: { color: '#8a90a2', fontSize: 13, paddingVertical: 6 },
  agent: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingHorizontal: 8,
    paddingVertical: 8,
    borderRadius: 8,
  },
  agentHover: { backgroundColor: '#1b1e27' },
  agentName: { color: '#d6d9e2', fontSize: 13.5, fontWeight: '600', width: 150 },
  agentWhere: { color: '#8a90a2', fontSize: 12.5, flex: 1 },
  badge: { fontSize: 11.5, fontWeight: '600', textTransform: 'capitalize' },
});
