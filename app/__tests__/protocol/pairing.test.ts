import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { SHARED_FIXTURES } from '../sharedFixtures';
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
  decodeServerKey,
} from '../../src/protocol';
import { fixture } from './fixtures';

describe('pairing URL', () => {
  // The v1 spec example plus the v2 fields.
  const pk = Buffer.alloc(32, 0xab).toString('base64url');
  const payload: PairingPayload = { ...fixture<PairingPayload>('pairing-payload'), v: 2, pk };

  it("parses the flockr server's v2 pairing fixture", () => {
    const server = JSON.parse(
      readFileSync(join(SHARED_FIXTURES, 'mobile-v2/pairing.payload.json'), 'utf8'),
    ) as PairingPayload;
    const b64 = Buffer.from(JSON.stringify(server), 'utf8').toString('base64url');
    expect(parsePairingUrl(`flockr://pair?p=${b64}`)).toEqual(server);
    expect(decodeServerKey(server.pk)).toHaveLength(32);
  });

  it('decodes pk to the 32-byte key', () => {
    expect(pk).toHaveLength(43);
    expect(Buffer.from(decodeServerKey(parsePairingUrl(encodePairingUrl(payload)).pk) ?? [])).toEqual(
      Buffer.alloc(32, 0xab),
    );
  });

  it('rejects v1 payloads with the update message', () => {
    expect(() => parsePairingUrl(encodePairingUrl({ ...payload, v: 1 }))).toThrow(
      'this QR is from an older flockr; update flockr and run `flockr mobile pair` again',
    );
    expect(() => parsePairingUrl(encodePairingUrl({ ...payload, v: 3 }))).toThrow(/older flockr/);
  });

  it('rejects a missing or malformed pk', () => {
    const { pk: _pk, ...noPk } = payload;
    const b64 = (o: unknown) => Buffer.from(JSON.stringify(o), 'utf8').toString('base64url');
    expect(() => parsePairingUrl(b64(noPk))).toThrow(/pk/);
    expect(() => parsePairingUrl(encodePairingUrl({ ...payload, pk: pk.slice(1) }))).toThrow(/pk/);
    expect(() => parsePairingUrl(encodePairingUrl({ ...payload, pk: `${pk.slice(0, 42)}!` }))).toThrow(/pk/);
    expect(() => parsePairingUrl(encodePairingUrl({ ...payload, pk: Buffer.alloc(31).toString('base64url') }))).toThrow(
      /pk/,
    );
  });

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
    expect(wsUrl({ host: '100.92.14.3', port: 47390 })).toBe('ws://100.92.14.3:47390/mobile/v2');
    expect(wsUrl({ host: 'fd00::1', port: 1 })).toBe('ws://[fd00::1]:1/mobile/v2');
  });
});
