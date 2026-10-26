// Expo push registration, guarded so the app still runs in Expo Go.
// Remote push needs a dev/preview build (expo-dev-client) on a real device.
import Constants, { ExecutionEnvironment } from 'expo-constants';
import * as Device from 'expo-device';
import { Platform } from 'react-native';
import type { PushData } from '../protocol';

type NotificationsModule = typeof import('expo-notifications');

export const isExpoGo = Constants.executionEnvironment === ExecutionEnvironment.StoreClient;

export function pushSupported(): boolean {
  return !isExpoGo && Device.isDevice && Platform.OS !== 'web';
}

let mod: NotificationsModule | null | undefined;

/** Load expo-notifications only where push works; Expo Go never loads it. */
function notifications(): NotificationsModule | null {
  if (mod !== undefined) return mod;
  if (!pushSupported()) {
    mod = null;
    return mod;
  }
  try {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    mod = require('expo-notifications') as NotificationsModule;
  } catch {
    mod = null;
  }
  return mod;
}

export function projectId(): string | undefined {
  const extra = Constants.expoConfig?.extra as { eas?: { projectId?: string } } | undefined;
  return extra?.eas?.projectId ?? Constants.easConfig?.projectId ?? undefined;
}

export interface PushSetup {
  token: string | null;
  /** Why there is no token, for the UI. */
  reason?: string;
}

let setupDone = false;

/** Set the foreground handler and Android channel once. */
export async function setupNotifications(): Promise<void> {
  const N = notifications();
  if (!N || setupDone) return;
  setupDone = true;
  N.setNotificationHandler({
    // The app shows in-band `notify` banners while in the foreground and the
    // server does not push then, so anything that still arrives is shown.
    handleNotification: async () => ({
      shouldShowBanner: true,
      shouldShowList: true,
      shouldPlaySound: true,
      shouldSetBadge: false,
    }),
  });
  if (Platform.OS === 'android') {
    await N.setNotificationChannelAsync('agents', {
      name: 'Agent alerts',
      importance: N.AndroidImportance.HIGH,
      vibrationPattern: [0, 200, 100, 200],
    });
  }
}

export async function getPushToken(): Promise<PushSetup> {
  if (isExpoGo) return { token: null, reason: 'Push needs a dev build (not Expo Go).' };
  if (!Device.isDevice) return { token: null, reason: 'Push needs a physical device.' };
  const N = notifications();
  if (!N) return { token: null, reason: 'expo-notifications not available.' };
  await setupNotifications();
  const existing = await N.getPermissionsAsync();
  let status = existing.status;
  if (status !== 'granted') {
    const asked = await N.requestPermissionsAsync({
      ios: { allowAlert: true, allowSound: true, allowBadge: false },
    });
    status = asked.status;
  }
  if (status !== 'granted') return { token: null, reason: 'Notification permission denied.' };
  const pid = projectId();
  if (!pid) return { token: null, reason: 'No EAS projectId in app.config.js.' };
  try {
    const t = await N.getExpoPushTokenAsync({ projectId: pid });
    return { token: t.data };
  } catch (e) {
    return { token: null, reason: `Could not get push token: ${String(e)}` };
  }
}

export function parsePushData(data: unknown): PushData | null {
  if (!data || typeof data !== 'object') return null;
  const d = data as Record<string, unknown>;
  if (typeof d.server_id !== 'string' || typeof d.pane_id !== 'string') return null;
  return d as unknown as PushData;
}

/** Call `onOpen` for notification taps, including the one that launched the app. */
export function listenForTaps(onOpen: (data: PushData) => void): () => void {
  const N = notifications();
  if (!N) return () => {};
  let cancelled = false;
  void N.getLastNotificationResponseAsync().then((resp) => {
    if (cancelled || !resp) return;
    const d = parsePushData(resp.notification.request.content.data);
    if (d) onOpen(d);
  });
  const sub = N.addNotificationResponseReceivedListener((resp) => {
    const d = parsePushData(resp.notification.request.content.data);
    if (d) onOpen(d);
  });
  return () => {
    cancelled = true;
    sub.remove();
  };
}
