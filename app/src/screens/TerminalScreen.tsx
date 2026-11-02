import React, { useCallback, useEffect, useRef, useState } from 'react';
import {
  Alert,
  AppState,
  Keyboard,
  KeyboardAvoidingView,
  Linking,
  Pressable,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { SafeAreaView, useSafeAreaInsets } from 'react-native-safe-area-context';
import { connections } from '../state/connections';
import { statusText, useAgents, useClientStatus } from '../state/hooks';
import { completionToMarkRead, isTransportError, MARK_READ_DELAY_MS } from '../state/markRead';
import type { Nav } from '../state/nav';
import { colors, ui } from '../state/ui';
import { GatewayClient, GatewayError, gridText, InputKind, linkAt, Subscription, SurfaceMode } from '../protocol';
import { canAttachImages, pickImage } from '../terminal/attachImage';
import { Composer, ComposerHandle } from '../terminal/Composer';
import { DesktopComposer } from '../desktop/DesktopComposer';
import { PerfOverlay } from '../terminal/PerfOverlay';
import { TextSheet } from '../terminal/TextSheet';
import { BASE_FONT_SIZE, makeViewStats, TerminalLayout, TerminalView } from '../terminal/TerminalView';

interface Props {
  nav: Nav;
  serverId: string;
  paneId: string;
  title: string;
  /** `desktop`: the wide web layout's main panel, with no back button. */
  variant?: 'phone' | 'desktop';
  /** Machine › workspace › tab, under the title on the desktop. */
  crumb?: string;
}

const FIT_FONT_SIZES = [8, 9, 10, 11, 12, 13, 14, 16];

export function TerminalScreen({ nav, serverId, paneId, title, variant = 'phone', crumb }: Props) {
  const client: GatewayClient | null = connections.client(serverId);
  const status = useClientStatus(client);
  const [mode, setMode] = useState<SurfaceMode>('fit');
  const [fitWidth, setFitWidth] = useState(true);
  const [zoom, setZoom] = useState(1);
  const [fitFont, setFitFont] = useState(variant === 'desktop' ? 13 : 11);
  const [perf, setPerf] = useState(false);
  const [nativeDebug, setNativeDebug] = useState(false);
  const [sub, setSub] = useState<Subscription | null>(null);
  // The pane's background, so the space around the terminal matches it.
  const [paneBg, setPaneBg] = useState<string | null>(null);
  const [ended, setEnded] = useState<string | null>(null);
  const [layout, setLayout] = useState<TerminalLayout | null>(null);
  const [sheet, setSheet] = useState<{ title: string; lines: string[] } | null>(null);
  const [lastError, setLastError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const viewStats = useRef(makeViewStats()).current;
  const insets = useSafeAreaInsets();
  const [keyboardOpen, setKeyboardOpen] = useState(false);
  const composer = useRef<ComposerHandle>(null);

  // Viewing a done agent marks it read on every device, once per completion.
  const { msg: agentsMsg } = useAgents(client);
  const [foreground, setForeground] = useState(AppState.currentState === 'active');
  useEffect(() => {
    const listener = AppState.addEventListener('change', (state) => setForeground(state === 'active'));
    return () => listener.remove();
  }, []);
  const markedSeq = useRef<number | null>(null);
  const agent = agentsMsg?.agents.find((a) => a.pane_id === paneId);
  const toMark = completionToMarkRead(agent, foreground, markedSeq.current);
  const online = status.state === 'open';
  useEffect(() => {
    if (!client || !online || toMark === null) return;
    const t = setTimeout(() => {
      client.api('pane.mark_read', { pane_id: paneId, completion_seq: toMark }).then(
        () => {
          markedSeq.current = toMark;
        },
        (e: unknown) => {
          // A server that answered, even with an error, is not asked again for this completion.
          if (!(e instanceof GatewayError && isTransportError(e.code))) markedSeq.current = toMark;
        },
      );
    }, MARK_READ_DELAY_MS);
    return () => clearTimeout(t);
  }, [client, online, paneId, toMark]);

  useEffect(() => {
    const show = Keyboard.addListener('keyboardDidShow', () => setKeyboardOpen(true));
    const hide = Keyboard.addListener('keyboardDidHide', () => setKeyboardOpen(false));
    return () => {
      show.remove();
      hide.remove();
    };
  }, []);

  // Phone grid size for fit mode.
  const fitZoom = fitFont / BASE_FONT_SIZE;
  const fitSize = layout
    ? {
        cols: Math.max(20, Math.floor(layout.width / (layout.metrics.cellW * fitZoom))),
        rows: Math.max(5, Math.floor(layout.height / (layout.metrics.cellH * fitZoom))),
      }
    : null;
  const fitKey = fitSize ? `${fitSize.cols}x${fitSize.rows}` : '';

  // (Re)subscribe when the mode changes. Fit size changes go through resize.
  useEffect(() => {
    if (!client) return;
    if (mode === 'fit' && !fitSize) return;
    setEnded(null);
    const s = client.subscribe(
      paneId,
      mode === 'fit' && fitSize ? { mode: 'fit', cols: fitSize.cols, rows: fitSize.rows } : { mode: 'observe' },
      {
        onStarted: () => setPaneBg(`#${s.theme.bg.toString(16).padStart(6, '0')}`),
        onEnded: (reason, message) => {
          // Another device (or a desktop attach) holds this pane's size: keep watching at the desktop size.
          if (mode === 'fit' && (reason === 'taken_over' || message?.startsWith('pane_busy'))) {
            setNotice('Another device has this pane in Fit, showing it at desktop size');
            setMode('observe');
            return;
          }
          setEnded(message ? `${reason}: ${message}` : reason);
        },
      },
    );
    setSub(s);
    return () => s.unsubscribe();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, paneId, mode, mode === 'fit' && !!fitSize]);

  // Fit mode: follow layout changes (rotation, keyboard) with a resize.
  useEffect(() => {
    if (!sub || mode !== 'fit' || !fitSize) return;
    const t = setTimeout(() => sub.resize(fitSize.cols, fitSize.rows), 150);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sub, mode, fitKey]);

  const send = useCallback(
    (kind: InputKind, value: string | string[]) => {
      if (!sub) return;
      sub.input(kind, value).catch((e: unknown) => setLastError(String((e as Error).message ?? e)));
    },
    [sub],
  );

  const openHistory = async () => {
    if (!sub) return;
    try {
      const lines = await sub.history(500);
      setSheet({ title: `History (${lines.length} lines)`, lines });
    } catch (e) {
      setLastError(String(e));
    }
  };

  const attachImage = useCallback(async () => {
    if (!sub) return;
    try {
      const image = await pickImage();
      if (!image) return;
      setNotice('Attaching image…');
      await sub.image(image.format, image.data);
      setNotice('Image attached');
    } catch (e) {
      setNotice(null);
      setLastError(String((e as Error).message ?? e));
    }
  }, [sub]);

  const onTapCell = useCallback(
    (col: number, row: number) => {
      composer.current?.tapTerminal();
      const url = sub ? linkAt(sub.grid, col, row) : null;
      if (!url) return;
      Alert.alert('Open link?', url, [
        { text: 'Cancel', style: 'cancel' },
        { text: 'Open', onPress: () => void Linking.openURL(url).catch((e: unknown) => setLastError(String(e))) },
      ]);
    },
    [sub],
  );

  const onLongPress = useCallback(() => {
    if (sub) setSheet({ title: 'Select text', lines: gridText(sub.grid) });
  }, [sub]);

  const onZoom = useCallback((z: number) => {
    setFitWidth(false);
    setZoom(z);
  }, []);

  const viewZoom = mode === 'fit' ? fitZoom : zoom;
  const viewFitWidth = mode === 'observe' && fitWidth;

  const terminal = (
    <TerminalView
      sub={sub}
      fitWidth={viewFitWidth}
      zoom={viewZoom}
      onZoom={mode === 'observe' ? onZoom : undefined}
      onLayout={setLayout}
      stats={viewStats}
      debug={nativeDebug}
      onTapCell={onTapCell}
      onLongPress={onLongPress}
    />
  );
  const fontDown = () => setFitFont(FIT_FONT_SIZES[Math.max(0, FIT_FONT_SIZES.indexOf(fitFont) - 1)]);
  const fontUp = () =>
    setFitFont(FIT_FONT_SIZES[Math.min(FIT_FONT_SIZES.length - 1, FIT_FONT_SIZES.indexOf(fitFont) + 1)]);
  const onAttach = client?.hasFeature('input.image') && canAttachImages() ? attachImage : undefined;

  if (variant === 'desktop') {
    const live = status.state === 'open';
    return (
      <View style={desk.main}>
        <View style={desk.header}>
          <View style={{ flex: 1, minWidth: 0 }}>
            <Text style={desk.title} numberOfLines={1}>
              {title}
            </Text>
            {crumb ? (
              <Text style={desk.crumb} numberOfLines={1}>
                {crumb}
              </Text>
            ) : null}
          </View>
          <View style={[desk.pill, live ? desk.pillLive : desk.pillOff]}>
            <View style={[desk.pillDot, { backgroundColor: live ? colors.ok : colors.warn }]} />
            <Text style={desk.pillText} numberOfLines={1}>
              {lastError ?? statusText(status)}
            </Text>
          </View>
          <View style={desk.tools}>
            <Ghost
              label={mode === 'fit' ? 'Fit' : 'Desktop size'}
              active={mode === 'fit'}
              hint="Fit the pane to this window, or watch it at the desktop's size"
              onPress={() => {
                setNotice(null);
                setMode(mode === 'fit' ? 'observe' : 'fit');
              }}
            />
            {mode === 'fit' ? (
              <>
                <Ghost label="A−" hint="Smaller text" onPress={fontDown} />
                <Ghost label="A+" hint="Larger text" onPress={fontUp} />
              </>
            ) : null}
            <Ghost label="History" hint="Scrollback as text" onPress={openHistory} />
            <Ghost label="↓" hint="Jump to the bottom" onPress={() => sub?.scrollToBottom()} />
          </View>
        </View>
        {ended || notice ? <Text style={desk.notice}>{ended ? `Stream ended: ${ended}` : notice}</Text> : null}
        <View style={[desk.surface, paneBg ? { backgroundColor: paneBg } : null]}>
          {perf ? <PerfOverlay client={client} sub={sub} view={viewStats} /> : null}
          {terminal}
        </View>
        <DesktopComposer
          ref={composer}
          target={title}
          disabled={!sub || !!ended}
          onSubmit={(t) => send('submit', t)}
          onPaste={(t) => send('paste', t)}
          onText={(t) => send('text', t)}
          onKeys={(k) => send('key', k)}
          onAttach={onAttach}
        />
        <TextSheet title={sheet?.title ?? ''} lines={sheet?.lines ?? null} onClose={() => setSheet(null)} />
      </View>
    );
  }

  return (
    // The avoiding view wraps the whole screen: its frame math assumes it starts at the top.
    // Android draws edge to edge, so the window no longer resizes for the keyboard: pad on both platforms.
    <KeyboardAvoidingView style={ui.screen} behavior="padding">
      <SafeAreaView style={{ flex: 1 }} edges={['top', 'left', 'right']}>
        <View style={ui.header}>
          <Pressable accessibilityRole="button" onPress={() => nav.pop()} style={ui.button}>
            <Text style={ui.buttonText}>‹</Text>
          </Pressable>
          <View style={{ flex: 1 }}>
            <Text style={ui.title} numberOfLines={1}>
              {title}
            </Text>
            <Text style={ui.sub} numberOfLines={1}>
              {statusText(status)}
              {lastError ? ` · ${lastError}` : ''}
            </Text>
          </View>
        </View>
        <View style={styles.toolbar}>
          <Tool
            label={mode === 'fit' ? 'Fit ✓' : 'Fit'}
            onPress={() => {
              setNotice(null);
              setMode(mode === 'fit' ? 'observe' : 'fit');
            }}
          />
          {mode === 'observe' ? (
            <Tool label={fitWidth ? 'Width' : `${zoom.toFixed(2)}x`} onPress={() => setFitWidth(!fitWidth)} />
          ) : (
            <>
              <Tool label="A−" onPress={fontDown} />
              <Tool label="A+" onPress={fontUp} />
            </>
          )}
          <Tool label="Bottom" onPress={() => sub?.scrollToBottom()} />
          <Tool label="History" onPress={openHistory} />
          <Tool
            label={perf ? 'Perf ✓' : 'Perf'}
            onPress={() => setPerf(!perf)}
            onLongPress={() => setNativeDebug(!nativeDebug)}
          />
        </View>
        {ended || notice ? <Text style={styles.banner}>{ended ? `Stream ended: ${ended}` : notice}</Text> : null}
        <View style={{ flex: 1 }}>
          {perf ? <PerfOverlay client={client} sub={sub} view={viewStats} /> : null}
          <View style={{ flex: 1 }}>{terminal}</View>
          <Composer
            ref={composer}
            disabled={!sub || !!ended}
            onSubmit={(t) => send('submit', t)}
            onPaste={(t) => send('paste', t)}
            onText={(t) => send('text', t)}
            onKeys={(k) => send('key', k)}
            bottomInset={keyboardOpen ? 0 : insets.bottom}
            keyboardOpen={keyboardOpen}
            onAttach={onAttach}
          />
        </View>
        <TextSheet title={sheet?.title ?? ''} lines={sheet?.lines ?? null} onClose={() => setSheet(null)} />
      </SafeAreaView>
    </KeyboardAvoidingView>
  );
}

function Tool({ label, onPress, onLongPress }: { label: string; onPress: () => void; onLongPress?: () => void }) {
  return (
    <Pressable accessibilityRole="button" onPress={onPress} onLongPress={onLongPress} style={[ui.button, styles.tool]}>
      <Text style={[ui.buttonText, { fontSize: 13 }]}>{label}</Text>
    </Pressable>
  );
}

function Ghost({
  label,
  hint,
  active,
  onPress,
}: {
  label: string;
  hint: string;
  active?: boolean;
  onPress: () => void;
}) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={hint}
      onPress={onPress}
      style={(state) => [
        desk.ghost,
        active && desk.ghostActive,
        (state as { hovered?: boolean }).hovered && desk.ghostHover,
      ]}
    >
      <Text style={[desk.ghostText, active && { color: colors.text }]}>{label}</Text>
    </Pressable>
  );
}

const desk = StyleSheet.create({
  main: { flex: 1, backgroundColor: colors.bg },
  header: {
    height: 60,
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 24,
    gap: 14,
    borderBottomWidth: 1,
    borderBottomColor: '#1e2029',
  },
  title: { color: colors.text, fontSize: 15.5, fontWeight: '650' as never },
  crumb: { color: '#6f7587', fontSize: 12.5, marginTop: 2 },
  pill: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 10,
    paddingVertical: 4,
    borderRadius: 999,
    borderWidth: 1,
    maxWidth: 260,
  },
  pillLive: { borderColor: '#25402f', backgroundColor: '#14201a' },
  pillOff: { borderColor: '#4a3c22', backgroundColor: '#211b10' },
  pillDot: { width: 6, height: 6, borderRadius: 3 },
  pillText: { color: '#aab0c0', fontSize: 12 },
  tools: { flexDirection: 'row', gap: 2 },
  ghost: { paddingHorizontal: 10, paddingVertical: 6, borderRadius: 8 },
  ghostActive: { backgroundColor: '#1d2029' },
  ghostHover: { backgroundColor: '#1a1d26' },
  ghostText: { color: '#8f95a7', fontSize: 13, fontWeight: '500' },
  notice: { color: colors.dim, fontSize: 12.5, paddingHorizontal: 24, paddingTop: 8 },
  surface: {
    flex: 1,
    overflow: 'hidden',
    backgroundColor: '#0d0e12',
    padding: 8,
  },
});

const styles = StyleSheet.create({
  banner: { color: colors.dim, fontSize: 12, paddingHorizontal: 12, paddingVertical: 6 },
  toolbar: { flexDirection: 'row', gap: 6, paddingHorizontal: 8, paddingVertical: 6, flexWrap: 'wrap' },
  tool: { paddingHorizontal: 10, paddingVertical: 6 },
});
