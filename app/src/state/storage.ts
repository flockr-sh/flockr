// Paired servers, stored in expo-secure-store (Keychain on iOS).
// One item per server plus an index of server ids.
import * as SecureStore from 'expo-secure-store';
import { Platform } from 'react-native';

export interface StoredServer {
  server_id: string;
  name: string;
  hosts: string[];
  port: number;
  device_id: string;
  token: string;
  /** Index into hosts of the last host that completed a handshake. */
  lastHost?: number;
  /** Token was rejected (revoked or unknown); the user must pair again. */
  needsRepair?: boolean;
  pairedAt: number;
}

const INDEX_KEY = 'flockr.servers';
const itemKey = (sid: string) => `flockr.server.${sid.replace(/[^A-Za-z0-9._-]/g, '_')}`;

// expo-secure-store has no web implementation; web is a desktop fallback only.
const web = Platform.OS === 'web';

async function getItem(key: string): Promise<string | null> {
  if (web) {
    try {
      return globalThis.localStorage?.getItem(key) ?? null;
    } catch {
      return null;
    }
  }
  return SecureStore.getItemAsync(key);
}

async function setItem(key: string, value: string): Promise<void> {
  if (web) {
    try {
      globalThis.localStorage?.setItem(key, value);
    } catch {
      // storage blocked
    }
    return;
  }
  await SecureStore.setItemAsync(key, value);
}

async function deleteItem(key: string): Promise<void> {
  if (web) {
    try {
      globalThis.localStorage?.removeItem(key);
    } catch {
      // storage blocked
    }
    return;
  }
  await SecureStore.deleteItemAsync(key);
}

async function readIndex(): Promise<string[]> {
  const raw = await getItem(INDEX_KEY);
  if (!raw) return [];
  try {
    const ids = JSON.parse(raw) as unknown;
    return Array.isArray(ids) ? ids.filter((x): x is string => typeof x === 'string') : [];
  } catch {
    return [];
  }
}

export async function loadServers(): Promise<StoredServer[]> {
  const ids = await readIndex();
  const out: StoredServer[] = [];
  for (const id of ids) {
    const raw = await getItem(itemKey(id));
    if (!raw) continue;
    try {
      out.push(JSON.parse(raw) as StoredServer);
    } catch {
      // skip a corrupt entry
    }
  }
  return out;
}

export async function saveServer(server: StoredServer): Promise<void> {
  await setItem(itemKey(server.server_id), JSON.stringify(server));
  const ids = await readIndex();
  if (!ids.includes(server.server_id)) {
    ids.push(server.server_id);
    await setItem(INDEX_KEY, JSON.stringify(ids));
  }
}

export async function deleteServer(serverId: string): Promise<void> {
  await deleteItem(itemKey(serverId));
  const ids = (await readIndex()).filter((x) => x !== serverId);
  await setItem(INDEX_KEY, JSON.stringify(ids));
}
