import {
  encodePairingUrl,
  PairingPayload,
  parseHostEntry,
  parsePairingUrl,
  utf8Decode,
  utf8Encode,
  utf8Length,
  wsUrl,
  base64UrlDecode,
  base64UrlEncode,
} from '../../src/protocol';
import { fixture } from './fixtures';

describe('pairing URL', () => {
  const payload = fixture<PairingPayload>('pairing-payload');

  it('round-trips the spec payload', () => {
    const url = encodePairingUrl(payload);
    expect(url.startsWith('flockr://pair?p=')).toBe(true);
    expect(url).not.toMatch(/[=+/]$/);
    expect(parsePairingUrl(url)).toEqual(payload);
  });

  it('matches Node base64url encoding', () => {
    const p = Buffer.from(JSON.stringify(payload), 'utf8').toString('base64url');
    expect(parsePairingUrl(`flockr://pair?p=${p}`)).toEqual(payload);
    expect(parsePairingUrl(p)).toEqual(payload);
    expect(parsePairingUrl(`  flockr://pair?p=${p}\n`)).toEqual(payload);
  });

  it('ignores unknown keys and query params', () => {
    const p = Buffer.from(JSON.stringify({ ...payload, extra: [1] }), 'utf8').toString('base64url');
    expect(parsePairingUrl(`flockr://pair?x=1&p=${p}`)).toEqual(payload);
  });

  it('rejects expired, wrong version, and junk', () => {
    expect(() => parsePairingUrl(encodePairingUrl(payload), payload.exp + 1)).toThrow(/expired/);
    expect(() => parsePairingUrl(encodePairingUrl({ ...payload, v: 2 }))).toThrow(/version/);
    expect(() => parsePairingUrl('https://example.com')).toThrow();
    expect(() => parsePairingUrl('flockr://pair?q=1')).toThrow(/p=/);
    expect(() => parsePairingUrl(encodePairingUrl({ ...payload, hosts: [] }))).toThrow(/hosts/);
  });

  it('handles non-ASCII names', () => {
    const p = { ...payload, name: 'Dylan’s 漢 🙂' };
    expect(parsePairingUrl(encodePairingUrl(p)).name).toBe(p.name);
  });
});

describe('utf8 + base64url', () => {
  it('matches Buffer', () => {
    const s = 'a€漢🙂';
    expect(Buffer.from(utf8Encode(s))).toEqual(Buffer.from(s, 'utf8'));
    expect(utf8Decode(Uint8Array.from(Buffer.from(s, 'utf8')))).toBe(s);
    expect(utf8Length(s)).toBe(Buffer.byteLength(s, 'utf8'));
    for (let n = 0; n < 40; n++) {
      const bytes = Uint8Array.from({ length: n }, (_, i) => (i * 37 + n) & 0xff);
      expect(base64UrlEncode(bytes)).toBe(Buffer.from(bytes).toString('base64url'));
      expect(Buffer.from(base64UrlDecode(base64UrlEncode(bytes)))).toEqual(Buffer.from(bytes));
    }
  });
});

describe('hosts', () => {
  it('parses host entries', () => {
    expect(parseHostEntry('100.92.14.3', 47390)).toEqual({ host: '100.92.14.3', port: 47390 });
    expect(parseHostEntry('100.92.14.3:5000', 47390)).toEqual({ host: '100.92.14.3', port: 5000 });
    expect(parseHostEntry('dk.netbird.cloud', 1)).toEqual({ host: 'dk.netbird.cloud', port: 1 });
    expect(parseHostEntry('fd00::1', 47390)).toEqual({ host: 'fd00::1', port: 47390 });
    expect(parseHostEntry('[fd00::1]:7', 47390)).toEqual({ host: 'fd00::1', port: 7 });
  });

  it('builds ws URLs', () => {
    expect(wsUrl({ host: '100.92.14.3', port: 47390 })).toBe('ws://100.92.14.3:47390/mobile/v1');
    expect(wsUrl({ host: 'fd00::1', port: 1 })).toBe('ws://[fd00::1]:1/mobile/v1');
  });
});
