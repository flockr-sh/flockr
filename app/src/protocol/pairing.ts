// Pairing URL decode (section 6) and host/URL helpers.
import { PairingPayload, WS_PATH } from './types';

const B64URL = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
const B64_LOOKUP: Record<string, number> = {};
for (let i = 0; i < B64URL.length; i++) B64_LOOKUP[B64URL[i]] = i;
// Accept standard base64 too, in case a copy/paste tool rewrote it.
B64_LOOKUP['+'] = 62;
B64_LOOKUP['/'] = 63;

export function base64UrlDecode(input: string): Uint8Array {
  const clean = input.replace(/=+$/, '');
  const out: number[] = [];
  let buffer = 0;
  let bits = 0;
  for (const ch of clean) {
    const v = B64_LOOKUP[ch];
    if (v === undefined) throw new Error(`invalid base64url character "${ch}"`);
    buffer = (buffer << 6) | v;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out.push((buffer >> bits) & 0xff);
    }
  }
  return Uint8Array.from(out);
}

export function base64UrlEncode(bytes: Uint8Array): string {
  let out = '';
  let buffer = 0;
  let bits = 0;
  for (const b of bytes) {
    buffer = (buffer << 8) | b;
    bits += 8;
    while (bits >= 6) {
      bits -= 6;
      out += B64URL[(buffer >> bits) & 0x3f];
    }
  }
  if (bits > 0) out += B64URL[(buffer << (6 - bits)) & 0x3f];
  return out;
}

export function utf8Decode(bytes: Uint8Array): string {
  let out = '';
  let i = 0;
  while (i < bytes.length) {
    const b0 = bytes[i++];
    let cp: number;
    if (b0 < 0x80) cp = b0;
    else if (b0 >> 5 === 0x6) cp = ((b0 & 0x1f) << 6) | (bytes[i++] & 0x3f);
    else if (b0 >> 4 === 0xe) cp = ((b0 & 0x0f) << 12) | ((bytes[i++] & 0x3f) << 6) | (bytes[i++] & 0x3f);
    else cp = ((b0 & 0x07) << 18) | ((bytes[i++] & 0x3f) << 12) | ((bytes[i++] & 0x3f) << 6) | (bytes[i++] & 0x3f);
    out += String.fromCodePoint(cp);
  }
  return out;
}

export function utf8Encode(text: string): Uint8Array {
  const out: number[] = [];
  for (const ch of text) {
    const cp = ch.codePointAt(0) ?? 0;
    if (cp < 0x80) out.push(cp);
    else if (cp < 0x800) out.push(0xc0 | (cp >> 6), 0x80 | (cp & 0x3f));
    else if (cp < 0x10000) out.push(0xe0 | (cp >> 12), 0x80 | ((cp >> 6) & 0x3f), 0x80 | (cp & 0x3f));
    else out.push(0xf0 | (cp >> 18), 0x80 | ((cp >> 12) & 0x3f), 0x80 | ((cp >> 6) & 0x3f), 0x80 | (cp & 0x3f));
  }
  return Uint8Array.from(out);
}

/** UTF-8 byte length of a string without allocating. */
export function utf8Length(text: string): number {
  let n = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (c < 0x80) n += 1;
    else if (c < 0x800) n += 2;
    else if (c >= 0xd800 && c <= 0xdbff) {
      n += 4;
      i++;
    } else n += 3;
  }
  return n;
}

export class PairingError extends Error {}

/**
 * Parse `flockr://pair?p=<base64url JSON>`. Also accepts the bare `p` value.
 * `nowSecs` is used to reject expired codes.
 */
export function parsePairingUrl(input: string, nowSecs?: number): PairingPayload {
  const text = input.trim();
  let p: string | null = null;
  const match = /^flockr:\/\/pair\?(.*)$/i.exec(text);
  if (match) {
    for (const part of match[1].split('&')) {
      const eq = part.indexOf('=');
      if (eq > 0 && part.slice(0, eq) === 'p') p = decodeURIComponent(part.slice(eq + 1));
    }
    if (!p) throw new PairingError('pairing URL has no p= parameter');
  } else if (/^[A-Za-z0-9_\-+/=]+$/.test(text)) {
    p = text;
  } else {
    throw new PairingError('not a flockr pairing URL');
  }
  let json: unknown;
  try {
    json = JSON.parse(utf8Decode(base64UrlDecode(p)));
  } catch {
    throw new PairingError('pairing payload is not valid base64url JSON');
  }
  return validatePayload(json, nowSecs);
}

function validatePayload(json: unknown, nowSecs?: number): PairingPayload {
  if (typeof json !== 'object' || json === null) throw new PairingError('payload is not an object');
  const o = json as Record<string, unknown>;
  if (o.v !== 1) throw new PairingError(`unsupported pairing version ${String(o.v)}`);
  const hosts = Array.isArray(o.hosts) ? o.hosts.filter((h): h is string => typeof h === 'string' && h.length > 0) : [];
  if (hosts.length === 0) throw new PairingError('payload has no hosts');
  if (typeof o.port !== 'number' || o.port < 1 || o.port > 65535) throw new PairingError('payload has no valid port');
  if (typeof o.sid !== 'string' || !o.sid) throw new PairingError('payload has no sid');
  if (typeof o.code !== 'string' || !o.code) throw new PairingError('payload has no code');
  const exp = typeof o.exp === 'number' ? o.exp : 0;
  if (nowSecs !== undefined && exp > 0 && exp < nowSecs) throw new PairingError('pairing code expired');
  return {
    v: 1,
    name: typeof o.name === 'string' && o.name ? o.name : hosts[0],
    hosts,
    port: o.port,
    sid: o.sid,
    code: o.code,
    exp,
  };
}

export function encodePairingUrl(payload: PairingPayload): string {
  return `flockr://pair?p=${base64UrlEncode(utf8Encode(JSON.stringify(payload)))}`;
}

export interface HostPort {
  host: string;
  port: number;
}

/**
 * Split a `hosts` entry. Entries may be `host`, `host:port`, `[v6]:port`, or a
 * bare IPv6 literal (no brackets, no port).
 */
export function parseHostEntry(entry: string, defaultPort: number): HostPort {
  const bracket = /^\[([^\]]+)\](?::(\d+))?$/.exec(entry);
  if (bracket) return { host: bracket[1], port: bracket[2] ? Number(bracket[2]) : defaultPort };
  const colons = entry.split(':').length - 1;
  if (colons === 1) {
    const [host, port] = entry.split(':');
    const n = Number(port);
    if (Number.isInteger(n) && n > 0 && n < 65536) return { host, port: n };
  }
  return { host: entry, port: defaultPort };
}

export function wsUrl(hp: HostPort): string {
  const host = hp.host.includes(':') ? `[${hp.host}]` : hp.host;
  return `ws://${host}:${hp.port}${WS_PATH}`;
}
