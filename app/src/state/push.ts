// Expo push registration, guarded so the app still runs in Expo Go.
// Remote push needs a dev/preview build (expo-dev-client) on a real device.
import Constants, { ExecutionEnvironment } from 'expo-constants';
import * as Device from 'expo-device';
import { Platform } from 'react-native';
import type { PushData } from '../protocol';
import { isPromptAction, OPEN_ACTION, registerPromptCategories, type PromptAction } from './pushActions';

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
    // Answer results are posted quietly.
    handleNotification: async (n) => ({
      shouldShowBanner: true,
      shouldShowList: true,
      shouldPlaySound: !(n.request.content.data as { quiet?: boolean } | undefined)?.quiet,
      shouldSetBadge: false,
    }),
  });
  try {
    await registerPromptCategories(N);
  } catch (e) {
    if (__DEV__) console.log(`notification categories failed: ${String(e)}`);
  }
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

/** A quiet local notification, such as the result of answering from an alert. */
export async function postLocalNotification(title: string, data: PushData): Promise<void> {
  const N = notifications();
  if (!N) return;
  await N.scheduleNotificationAsync({
    content: {
      title,
      sound: false,
      // A tap opens the pane like the alert did.
      data: { ...data, type: 'answer_result', prompt: undefined, quiet: true },
    },
    trigger: null,
  });
}

// Responses already handled, so the launch response and the listener (or a
// remount) never answer twice.
const handled = new Set<string>();

/**
 * Notification responses, including the one that launched the app: a tap or
 * Open calls `onOpen`, an answer button calls `onAnswer`.
 */
export function listenForTaps(
  onOpen: (data: PushData) => void,
  onAnswer: (data: PushData, action: PromptAction) => void,
): () => void {
  const N = notifications();
  if (!N) return () => {};
  let cancelled = false;
  const handle = (resp: import('expo-notifications').NotificationResponse) => {
    const key = `${resp.notification.request.identifier}\u0000${resp.actionIdentifier}`;
    if (handled.has(key)) return;
    handled.add(key);
    try {
      N.clearLastNotificationResponse();
    } catch {
      // Older native module: the handled set still guards this run.
    }
    const d = parsePushData(resp.notification.request.content.data);
    if (!d) return;
    const action = resp.actionIdentifier;
    if (isPromptAction(action)) onAnswer(d, action);
    else if (action === OPEN_ACTION || action === N.DEFAULT_ACTION_IDENTIFIER) onOpen(d);
  };
  void N.getLastNotificationResponseAsync().then((resp) => {
    if (!cancelled && resp) handle(resp);
  });
  const sub = N.addNotificationResponseReceivedListener(handle);
  return () => {
    cancelled = true;
    sub.remove();
  };
}
