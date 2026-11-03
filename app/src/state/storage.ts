// Paired servers, stored in expo-secure-store (Keychain on iOS).
// One item per server plus an index of server ids.
import * as SecureStore from 'expo-secure-store';
import { Platform } from 'react-native';
import { decodeServerKey } from '../protocol/pairing';

export interface StoredServer {
  server_id: string;
  name: string;
  hosts: string[];
  port: number;
  /** Server Noise public key from the QR (base64url, 43 chars). Missing on records paired under v1. */
  pk?: string;
  device_id: string;
  token: string;
  /** Index into hosts of the last host that completed a handshake. */
  lastHost?: number;
  /** Token was rejected (revoked or unknown); the user must pair again. */
  needsRepair?: boolean;
  /** The Noise handshake failed: the server key changed or the address is another machine. The token is kept. */
  keyMismatch?: boolean;
  pairedAt: number;
  /** A flock machine reached through the server `via`, using that server's pairing. */
  machine?: string;
  /** The paired server whose flock this machine belongs to. */
  via?: string;
  /** A made-up machine from demo mode. Kept in memory only, never stored. */
  demo?: boolean;
}

export type RepairReason = 'token' | 'key' | 'legacy';

/** Why this server cannot connect until the phone pairs again, or null if it can. */
export function repairReason(s: StoredServer): RepairReason | null {
  if (s.needsRepair) return 'token';
  if (!decodeServerKey(s.pk)) return 'legacy';
  if (s.keyMismatch) return 'key';
  return null;
}

export function repairMessage(reason: RepairReason): string {
  switch (reason) {
    case 'token':
      return 'This phone was revoked or its token is no longer valid.';
    case 'key':
      return "This server's key changed or the address is a different machine. Pair again.";
    case 'legacy':
      return 'This server was paired with an older flockr without encryption. Pair again.';
  }
}

export function repairShort(reason: RepairReason): string {
  switch (reason) {
    case 'token':
      return 'token rejected: pair again';
    case 'key':
      return 'server key changed: pair again';
    case 'legacy':
      return 'needs re-pair';
  }
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

const DEMO_KEY = 'flockr.demo';

/** Whether demo mode was on when the app last ran. */
export async function loadDemoFlag(): Promise<boolean> {
  return (await getItem(DEMO_KEY)) === '1';
}

export async function saveDemoFlag(on: boolean): Promise<void> {
  if (on) await setItem(DEMO_KEY, '1');
  else await deleteItem(DEMO_KEY);
}
