import * as Linking from 'expo-linking';
import { useFonts } from 'expo-font';
import { StatusBar } from 'expo-status-bar';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AppState, BackHandler, Platform, Pressable, StyleSheet, Text, useWindowDimensions, View } from 'react-native';
import { SafeAreaProvider, SafeAreaView } from 'react-native-safe-area-context';
import { connections, InbandNotify } from './src/state/connections';
import type { Nav, Route } from './src/state/nav';
import { getPushToken, listenForTaps, pushSupported, setupNotifications } from './src/state/push';
import { colors, statusColor } from './src/state/ui';
import type { PushData } from './src/protocol';
import { DesktopShell } from './src/desktop/DesktopShell';
import { AgentsScreen } from './src/screens/AgentsScreen';
import { PairScreen } from './src/screens/PairScreen';
import { ServersScreen } from './src/screens/ServersScreen';
import { TerminalScreen } from './src/screens/TerminalScreen';

export default function App() {
  const [ready, setReady] = useState(false);
  // Plain Text that shows terminal output (history, perf) needs the same glyph coverage as the Skia view.
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const [fontsLoaded, fontError] = useFonts({ JuliaMono: require('./assets/fonts/JuliaMono-Regular.ttf') });
  const [stack, setStack] = useState<Route[]>([{ name: 'servers' }]);
  const [banner, setBanner] = useState<InbandNotify | null>(null);
  const [pushNote, setPushNote] = useState<string | null>(null);
  // A wide browser window gets the desktop layout; phones and narrow windows the stack.
  const { width } = useWindowDimensions();
  const desktop = Platform.OS === 'web' && width >= 860;
  const bannerTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const nav: Nav = useMemo(
    () => ({
      push: (r) => setStack((s) => [...s, r]),
      pop: () => setStack((s) => (s.length > 1 ? s.slice(0, -1) : s)),
      reset: (routes) => setStack(routes.length ? routes : [{ name: 'servers' }]),
    }),
    [],
  );

  useEffect(() => {
    void connections.load().then(() => setReady(true));
  }, []);

  // Foreground/background: tells servers whether to push or notify in-band.
  useEffect(() => {
    connections.setForeground(AppState.currentState === 'active');
    const sub = AppState.addEventListener('change', (s) => connections.setForeground(s === 'active'));
    return () => sub.remove();
  }, []);

  useEffect(() => {
    const sub = BackHandler.addEventListener('hardwareBackPress', () => {
      if (stack.length > 1) {
        nav.pop();
        return true;
      }
      return false;
    });
    return () => sub.remove();
  }, [stack, nav]);

  const openPane = useCallback(
    (serverId: string, paneId: string, title: string) => {
      if (!connections.server(serverId)) return;
      nav.reset([{ name: 'servers' }, { name: 'agents', serverId }, { name: 'terminal', serverId, paneId, title }]);
    },
    [nav],
  );

  // Push: register a token (dev builds only) and open the pane on tap.
  useEffect(() => {
    if (!ready) return;
    if (!pushSupported()) {
      setPushNote('Push needs a dev build on a real device.');
      return;
    }
    void setupNotifications();
    void getPushToken().then((r) => {
      connections.setPushToken(r.token);
      setPushNote(r.token ? 'Push notifications on.' : (r.reason ?? null));
    });
    return listenForTaps((d: PushData) =>
      openPane(d.server_id, d.pane_id, `${d.agent ?? 'Agent'} · ${d.workspace ?? ''} › ${d.tab ?? ''}`),
    );
  }, [ready, openPane]);

  // In-band alerts while the app is open.
  useEffect(
    () =>
      connections.onNotify((n) => {
        setBanner(n);
        if (bannerTimer.current) clearTimeout(bannerTimer.current);
        bannerTimer.current = setTimeout(() => setBanner(null), 6000);
      }),
    [],
  );

  // flockr://pair?p=... links open the pair screen.
  const url = Linking.useURL();
  useEffect(() => {
    if (url && url.startsWith('flockr://pair')) nav.push({ name: 'pair', initialUrl: url });
  }, [url, nav]);

  const route = stack[stack.length - 1];
  let screen: React.ReactNode = null;
  if (!ready || (!fontsLoaded && !fontError)) screen = <View style={{ flex: 1, backgroundColor: colors.bg }} />;
  else if (desktop) screen = <DesktopShell />;
  else if (route.name === 'servers') screen = <ServersScreen nav={nav} pushNote={pushNote} />;
  else if (route.name === 'pair') screen = <PairScreen nav={nav} initialUrl={route.initialUrl} />;
  else if (route.name === 'agents') screen = <AgentsScreen key={route.serverId} nav={nav} serverId={route.serverId} />;
  else
    screen = (
      <TerminalScreen
        key={`${route.serverId}/${route.paneId}`}
        nav={nav}
        serverId={route.serverId}
        paneId={route.paneId}
        title={route.title}
      />
    );

  return (
    <SafeAreaProvider>
      <StatusBar style="light" />
      {screen}
      {banner && !desktop ? (
        <SafeAreaView edges={['top']} style={styles.bannerWrap} pointerEvents="box-none">
          <Pressable
            accessibilityRole="button"
            style={[styles.banner, { borderLeftColor: statusColor[banner.kind === 'blocked' ? 'blocked' : 'done'] }]}
            onPress={() => {
              setBanner(null);
              openPane(banner.server_id, banner.pane_id, banner.title);
            }}
          >
            <Text style={styles.bannerTitle}>{banner.title}</Text>
            <Text style={styles.bannerBody}>{banner.body}</Text>
          </Pressable>
        </SafeAreaView>
      ) : null}
    </SafeAreaProvider>
  );
}

const styles = StyleSheet.create({
  bannerWrap: { position: 'absolute', top: 0, left: 0, right: 0 },
  banner: {
    margin: 8,
    padding: 12,
    borderRadius: 10,
    backgroundColor: colors.panelHi,
    borderLeftWidth: 4,
    shadowColor: '#000',
    shadowOpacity: 0.4,
    shadowRadius: 8,
    elevation: 6,
  },
  bannerTitle: { color: colors.text, fontWeight: '700', fontSize: 15 },
  bannerBody: { color: colors.dim, fontSize: 13, marginTop: 2 },
});
