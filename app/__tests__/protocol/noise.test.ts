import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import {
  CipherState,
  FrameError,
  keyPairFromPrivate,
  MAX_CHUNK,
  MessageReader,
  NOISE_PROLOGUE,
  NoiseInitiator,
  NoiseResponder,
  sealBytes,
  sealMessage,
  TransportCiphers,
} from '../../src/protocol/noise';

const hex = (s: string) => Uint8Array.from(Buffer.from(s, 'hex'));
const toHex = (b: Uint8Array) => Buffer.from(b).toString('hex');

interface Vector {
  protocol_name: string;
  init_prologue: string;
  init_ephemeral: string;
  init_remote_static: string;
  resp_static: string;
  resp_ephemeral: string;
  handshake_hash?: string;
  messages: { payload: string; ciphertext: string }[];
  source: string;
}

// Noise_NK_25519_ChaChaPoly_SHA256 vectors copied from
//   https://raw.githubusercontent.com/haskell-cryptography/cacophony/master/vectors/cacophony.txt
//   https://raw.githubusercontent.com/mcginty/snow/main/tests/vectors/snow.txt
// (the `source` field of each entry says which).
const vectors = (
  JSON.parse(readFileSync(join(__dirname, '../fixtures/noise-nk-vectors.json'), 'utf8')) as { vectors: Vector[] }
).vectors;

function runVector(v: Vector) {
  const prologue = hex(v.init_prologue);
  const resp = keyPairFromPrivate(hex(v.resp_static));
  expect(toHex(resp.publicKey)).toBe(v.init_remote_static);
  const init = new NoiseInitiator(hex(v.init_remote_static), { prologue, ephemeral: hex(v.init_ephemeral) });
  const responder = new NoiseResponder(resp, { prologue, ephemeral: hex(v.resp_ephemeral) });

  const [m1, m2, ...transport] = v.messages;
  const msg1 = init.writeMessage1(hex(m1.payload));
  expect(toHex(msg1)).toBe(m1.ciphertext);
  expect(toHex(responder.readMessage1(msg1))).toBe(m1.payload);

  const { message: msg2, ciphers: rc } = responder.writeMessage2(hex(m2.payload));
  expect(toHex(msg2)).toBe(m2.ciphertext);
  const { payload, ciphers: ic } = init.readMessage2(msg2);
  expect(toHex(payload)).toBe(m2.payload);
  if (v.handshake_hash) expect(toHex(ic.handshakeHash)).toBe(v.handshake_hash);
  expect(toHex(rc.handshakeHash)).toBe(toHex(ic.handshakeHash));

  // Transport messages alternate, initiator first.
  transport.forEach((m, i) => {
    const [from, to] = i % 2 === 0 ? [ic, rc] : [rc, ic];
    const ct = from.send.encrypt(hex(m.payload));
    expect(toHex(ct)).toBe(m.ciphertext);
    expect(toHex(to.recv.decrypt(ct))).toBe(m.payload);
  });
}

describe('Noise_NK_25519_ChaChaPoly_SHA256', () => {
  it.each(vectors.map((v) => [v.source, v] as const))('matches the vector from %s', (_src, v) => {
    expect(v.protocol_name).toBe('Noise_NK_25519_ChaChaPoly_SHA256');
    runVector(v);
  });

  it('prologue is the 16 ASCII bytes flockr.mobile.v2', () => {
    expect(Buffer.from(NOISE_PROLOGUE).toString('latin1')).toBe('flockr.mobile.v2');
    expect(NOISE_PROLOGUE.length).toBe(16);
  });

  it('handshake messages are 48 bytes with empty payloads', () => {
    const s = keyPairFromPrivate(new Uint8Array(32).fill(7));
    const init = new NoiseInitiator(s.publicKey);
    const resp = new NoiseResponder(s);
    const m1 = init.writeMessage1();
    expect(m1.length).toBe(48);
    resp.readMessage1(m1);
    const { message } = resp.writeMessage2();
    expect(message.length).toBe(48);
    init.readMessage2(message);
  });

  it('a wrong server key fails the handshake on the responder', () => {
    const real = keyPairFromPrivate(new Uint8Array(32).fill(1));
    const pinned = keyPairFromPrivate(new Uint8Array(32).fill(2)).publicKey;
    const init = new NoiseInitiator(pinned);
    const resp = new NoiseResponder(real);
    expect(() => resp.readMessage1(init.writeMessage1())).toThrow(/decrypt/);
  });

  it('a tampered message 2 fails on the initiator', () => {
    const s = keyPairFromPrivate(new Uint8Array(32).fill(3));
    const init = new NoiseInitiator(s.publicKey);
    const resp = new NoiseResponder(s);
    resp.readMessage1(init.writeMessage1());
    const { message } = resp.writeMessage2();
    message[40] ^= 1;
    expect(() => init.readMessage2(message)).toThrow(/decrypt/);
  });
});

// Cross-implementation vector from the flockr server (generated with snow).
// Copied from flockr's tests/fixtures/mobile-v2/noise-nk.json.
const serverFixture = join(__dirname, '../fixtures/mobile-v2/noise-nk.json');
const haveServerFixture = existsSync(serverFixture);
if (!haveServerFixture) {
  console.warn(
    `skipping the flockr server Noise replay: ${serverFixture} is missing. Copy it from flockr's ` +
      'tests/fixtures/mobile-v2/noise-nk.json once the server side lands.',
  );
}

(haveServerFixture ? describe : describe.skip)('flockr server fixture replay (mobile-v2/noise-nk.json)', () => {
  interface ServerFixture {
    protocol: string;
    prologue_hex: string;
    responder_static_private_hex: string;
    responder_static_public_hex: string;
    initiator_ephemeral_private_hex: string;
    responder_ephemeral_private_hex: string;
    msg1_hex: string;
    msg2_hex: string;
    handshake_hash_hex: string;
    transport: { from: 'initiator' | 'responder'; plaintext_hex: string; ciphertext_hex: string }[];
  }

  it('replays byte for byte as the initiator', () => {
    const f = JSON.parse(readFileSync(serverFixture, 'utf8')) as ServerFixture;
    expect(f.protocol).toBe('Noise_NK_25519_ChaChaPoly_SHA256');
    expect(f.prologue_hex).toBe(toHex(NOISE_PROLOGUE));
    expect(toHex(keyPairFromPrivate(hex(f.responder_static_private_hex)).publicKey)).toBe(
      f.responder_static_public_hex,
    );
    const init = new NoiseInitiator(hex(f.responder_static_public_hex), {
      ephemeral: hex(f.initiator_ephemeral_private_hex),
    });
    expect(toHex(init.writeMessage1())).toBe(f.msg1_hex);
    const { payload, ciphers } = init.readMessage2(hex(f.msg2_hex));
    expect(payload.length).toBe(0);
    expect(toHex(ciphers.handshakeHash)).toBe(f.handshake_hash_hex);
    for (const m of f.transport) {
      if (m.from === 'initiator') {
        expect(toHex(ciphers.send.encrypt(hex(m.plaintext_hex)))).toBe(m.ciphertext_hex);
      } else {
        expect(toHex(ciphers.recv.decrypt(hex(m.ciphertext_hex)))).toBe(m.plaintext_hex);
      }
    }
  });
});

function pair(): { a: TransportCiphers; b: TransportCiphers } {
  const s = keyPairFromPrivate(new Uint8Array(32).fill(9));
  const init = new NoiseInitiator(s.publicKey);
  const resp = new NoiseResponder(s);
  resp.readMessage1(init.writeMessage1());
  const r = resp.writeMessage2();
  return { a: init.readMessage2(r.message).ciphers, b: r.ciphers };
}

describe('v2 framing', () => {
  it('one frame for a small message, flag 0', () => {
    const { a, b } = pair();
    const frames = sealMessage(a.send, '{"t":"ping","ts":1}');
    expect(frames).toHaveLength(1);
    expect(b.recv.decrypt(frames[0])[0]).toBe(0);
  });

  it('chunks at 65518 bytes and reassembles', () => {
    const { a, b } = pair();
    const text = JSON.stringify({ t: 'x', s: 'é'.repeat(100000) });
    const frames = sealMessage(a.send, text);
    const size = Buffer.byteLength(text);
    expect(frames).toHaveLength(Math.ceil(size / MAX_CHUNK));
    for (const f of frames) expect(f.length).toBeLessThanOrEqual(65535);
    expect(frames[0].length).toBe(65535);
    const reader = new MessageReader(b.recv, 4 * 1024 * 1024);
    const out = frames.map((f) => reader.push(f));
    expect(out.slice(0, -1).every((x) => x === null)).toBe(true);
    expect(out[out.length - 1]).toBe(text);
    expect(reader.lastSize).toBe(size);
  });

  it('an exact multiple of the chunk size ends with a full last chunk', () => {
    const { a, b } = pair();
    const bytes = new Uint8Array(MAX_CHUNK * 2).fill(0x61);
    const frames = sealBytes(a.send, bytes);
    expect(frames).toHaveLength(2);
    const reader = new MessageReader(b.recv, 1 << 20);
    expect(reader.push(frames[0])).toBeNull();
    expect(reader.push(frames[1])?.length).toBe(MAX_CHUNK * 2);
  });

  it('stops at the cap while reassembling (1009)', () => {
    const { a, b } = pair();
    const frames = sealBytes(a.send, new Uint8Array(200000).fill(0x61));
    const reader = new MessageReader(b.recv, 100000);
    expect(reader.push(frames[0])).toBeNull();
    let err: unknown;
    try {
      reader.push(frames[1]);
    } catch (e) {
      err = e;
    }
    expect(err).toBeInstanceOf(FrameError);
    expect((err as FrameError).closeCode).toBe(1009);
  });

  it('unknown flag and bad ciphertext are protocol errors (4000)', () => {
    const { a, b } = pair();
    const reader = new MessageReader(b.recv, 1 << 20);
    const bad = a.send.encrypt(Uint8Array.of(2, 0x7b, 0x7d));
    expect(() => reader.push(bad)).toThrow(expect.objectContaining({ closeCode: 4000 }));
    const reader2 = new MessageReader(new CipherState(new Uint8Array(32)), 1 << 20);
    expect(() => reader2.push(new Uint8Array(40))).toThrow(expect.objectContaining({ closeCode: 4000 }));
  });
});

describe('crypto cost (Node, informational)', () => {
  it('measures 64 KiB chunk throughput and a 2 KiB delta', () => {
    const { a, b } = pair();
    const reader = new MessageReader(b.recv, 4 * 1024 * 1024);
    const big = JSON.stringify({ t: 'surface.frame', s: 'x'.repeat(MAX_CHUNK - 30) });
    const small = JSON.stringify({ t: 'surface.delta', s: 'y'.repeat(2048 - 30) });
    const run = (text: string, n: number) => {
      for (let i = 0; i < 5; i++) for (const f of sealMessage(a.send, text)) reader.push(f);
      const t0 = performance.now();
      for (let i = 0; i < n; i++) for (const f of sealMessage(a.send, text)) reader.push(f);
      return (performance.now() - t0) / n;
    };
    const bigMs = run(big, 200);
    const smallMs = run(small, 2000);
    const mbps = Buffer.byteLength(big) / 1024 / 1024 / (bigMs / 1000);
    console.log(
      `noise bench: 64 KiB encrypt+decrypt ${bigMs.toFixed(3)} ms (${mbps.toFixed(1)} MiB/s); ` +
        `2 KiB delta encrypt+decrypt ${(smallMs * 1000).toFixed(1)} µs`,
    );
    expect(smallMs).toBeLessThan(1);
  });
});
