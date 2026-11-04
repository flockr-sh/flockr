import * as Linking from 'expo-linking';
import { useFonts } from 'expo-font';
import { StatusBar } from 'expo-status-bar';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AppState, BackHandler, Platform, Pressable, StyleSheet, Text, useWindowDimensions, View } from 'react-native';
import { SafeAreaProvider, SafeAreaView } from 'react-native-safe-area-context';
import { connections, InbandNotify } from './src/state/connections';
import type { Nav, Route } from './src/state/nav';
import {
  getPushToken,
  listenForTaps,
  postLocalNotification,
  pushSupported,
  setupNotifications,
} from './src/state/push';
import { answerFromPush } from './src/state/pushActions';
import { colors, statusColor } from './src/state/ui';
import type { PushData } from './src/protocol';
import { DesktopShell } from './src/desktop/DesktopShell';
import { AgentsScreen } from './src/screens/AgentsScreen';
import { PairScreen } from './src/screens/PairScreen';
import { MessagesScreen } from './src/screens/MessagesScreen';
import { ServersScreen } from './src/screens/ServersScreen';
import { SettingsScreen } from './src/screens/SettingsScreen';
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
      // Only Expo Go and simulators land here; that's a developer's concern.
      if (__DEV__) setPushNote('Push needs a dev build on a real device.');
      return;
    }
    void setupNotifications();
    void getPushToken().then((r) => {
      connections.setPushToken(r.token);
      setPushNote(r.token ? 'Push notifications on.' : (r.reason ?? null));
    });
    const open = (d: PushData) =>
      openPane(d.server_id, d.pane_id, `${d.agent ?? 'Agent'} · ${d.workspace ?? ''} › ${d.tab ?? ''}`);
    return listenForTaps(open, (d, action) => {
      // Answer buttons open the app (see pushActions.ts); answer now, and
      // show the pane if the prompt could not be answered.
      void answerFromPush(
        {
          client: (id) => connections.client(id),
          isDemo: (id) => !!connections.server(id)?.demo,
          notify: postLocalNotification,
        },
        d,
        action,
      ).then((r) => {
        if (r !== 'sent') open(d);
      });
    });
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
  else if (route.name === 'messages') screen = <MessagesScreen nav={nav} />;
  else if (route.name === 'settings') screen = <SettingsScreen nav={nav} />;
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
            style={({ pressed }) => [styles.banner, pressed && styles.bannerPressed]}
            onPress={() => {
              setBanner(null);
              openPane(banner.server_id, banner.pane_id, banner.title);
            }}
          >
            <View
              style={[styles.bannerDot, { backgroundColor: statusColor[banner.kind === 'blocked' ? 'blocked' : 'done'] }]}
            />
            <View style={styles.bannerText}>
              <Text style={styles.bannerTitle} numberOfLines={1}>
                {banner.title}
              </Text>
              <Text style={styles.bannerBody} numberOfLines={1}>
                {banner.body}
              </Text>
            </View>
            <Pressable
              accessibilityRole="button"
              accessibilityLabel="Dismiss"
              hitSlop={8}
              onPress={() => setBanner(null)}
              style={styles.bannerClose}
            >
              <Text style={styles.bannerCloseText}>×</Text>
            </Pressable>
          </Pressable>
        </SafeAreaView>
      ) : null}
    </SafeAreaProvider>
  );
}

const styles = StyleSheet.create({
  bannerWrap: { position: 'absolute', top: 0, left: 0, right: 0 },
  banner: {
    marginHorizontal: 8,
    marginTop: 4,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    paddingLeft: 12,
    paddingRight: 6,
    paddingVertical: 10,
    borderRadius: 10,
    backgroundColor: colors.panelHi,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: colors.border,
    shadowColor: '#000',
    shadowOpacity: 0.45,
    shadowRadius: 16,
    shadowOffset: { width: 0, height: 6 },
    elevation: 6,
  },
  bannerPressed: { backgroundColor: colors.panel },
  bannerDot: { width: 10, height: 10, borderRadius: 5 },
  bannerText: { flex: 1, minWidth: 0 },
  bannerTitle: { color: colors.text, fontSize: 15, fontWeight: '600' },
  bannerBody: { color: colors.dim, fontSize: 13, marginTop: 2 },
  bannerClose: { width: 30, height: 30, alignItems: 'center', justifyContent: 'center' },
  bannerCloseText: { color: colors.dim, fontSize: 20, lineHeight: 22 },
});
