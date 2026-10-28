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
import { PerfOverlay } from '../terminal/PerfOverlay';
import { TextSheet } from '../terminal/TextSheet';
import { BASE_FONT_SIZE, makeViewStats, TerminalLayout, TerminalView } from '../terminal/TerminalView';

interface Props {
  nav: Nav;
  serverId: string;
  paneId: string;
  title: string;
}

const FIT_FONT_SIZES = [8, 9, 10, 11, 12, 13, 14, 16];

export function TerminalScreen({ nav, serverId, paneId, title }: Props) {
  const client: GatewayClient | null = connections.client(serverId);
  const status = useClientStatus(client);
  const [mode, setMode] = useState<SurfaceMode>('fit');
  const [fitWidth, setFitWidth] = useState(true);
  const [zoom, setZoom] = useState(1);
  const [fitFont, setFitFont] = useState(11);
  const [perf, setPerf] = useState(false);
  const [nativeDebug, setNativeDebug] = useState(false);
  const [sub, setSub] = useState<Subscription | null>(null);
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
              <Tool
                label="A−"
                onPress={() => setFitFont(FIT_FONT_SIZES[Math.max(0, FIT_FONT_SIZES.indexOf(fitFont) - 1)])}
              />
              <Tool
                label="A+"
                onPress={() =>
                  setFitFont(FIT_FONT_SIZES[Math.min(FIT_FONT_SIZES.length - 1, FIT_FONT_SIZES.indexOf(fitFont) + 1)])
                }
              />
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
          <View style={{ flex: 1 }}>
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
          </View>
          <Composer
            ref={composer}
            disabled={!sub || !!ended}
            onSubmit={(t) => send('submit', t)}
            onPaste={(t) => send('paste', t)}
            onText={(t) => send('text', t)}
            onKeys={(k) => send('key', k)}
            bottomInset={keyboardOpen ? 0 : insets.bottom}
            keyboardOpen={keyboardOpen}
            onAttach={client?.hasFeature('input.image') && canAttachImages() ? attachImage : undefined}
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

const styles = StyleSheet.create({
  banner: { color: colors.dim, fontSize: 12, paddingHorizontal: 12, paddingVertical: 6 },
  toolbar: { flexDirection: 'row', gap: 6, paddingHorizontal: 8, paddingVertical: 6, flexWrap: 'wrap' },
  tool: { paddingHorizontal: 10, paddingVertical: 6 },
});
