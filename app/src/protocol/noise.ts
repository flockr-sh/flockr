// Noise_NK_25519_ChaChaPoly_SHA256 (Noise spec rev 34) and the mobile.v2
// transport framing. Pure JS on @noble; runs in Hermes and Node.
// The phone is the initiator. `NoiseResponder` is here so the mock gateway
// and tests can speak the same protocol; the app never uses it.
import { chacha20poly1305 } from '@noble/ciphers/chacha.js';
import { x25519 } from '@noble/curves/ed25519.js';
import { hmac } from '@noble/hashes/hmac.js';
import { sha256 } from '@noble/hashes/sha2.js';
import { utf8Decode, utf8Encode } from './pairing';

export const NOISE_PROTOCOL = 'Noise_NK_25519_ChaChaPoly_SHA256';
/** ASCII `flockr.mobile.v2`. */
export const NOISE_PROLOGUE = asciiBytes('flockr.mobile.v2');
const DHLEN = 32;
const TAGLEN = 16;
/** Noise message 1 and 2 with empty payloads: 32-byte key + 16-byte tag. */
export const HANDSHAKE_MSG_LEN = DHLEN + TAGLEN;
/** Largest Noise message. */
export const NOISE_MAX_MESSAGE = 65535;
/** Largest chunk per transport frame: flag byte + chunk + tag = 65535. */
export const MAX_CHUNK = NOISE_MAX_MESSAGE - TAGLEN - 1;
const EMPTY = new Uint8Array(0);

export class NoiseError extends Error {}

function asciiBytes(s: string): Uint8Array {
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
  return out;
}

function concat(...parts: Uint8Array[]): Uint8Array {
  let n = 0;
  for (const p of parts) n += p.length;
  const out = new Uint8Array(n);
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
}

/** CSPRNG bytes from WebCrypto (Node, or the expo-crypto polyfill in the app). */
export function randomBytes(n: number): Uint8Array {
  const c = (globalThis as { crypto?: { getRandomValues?: (a: Uint8Array) => Uint8Array } }).crypto;
  if (!c?.getRandomValues) throw new NoiseError('no crypto.getRandomValues in this runtime');
  const out = new Uint8Array(n);
  c.getRandomValues(out);
  return out;
}

export interface KeyPair {
  privateKey: Uint8Array;
  publicKey: Uint8Array;
}

export function keyPairFromPrivate(privateKey: Uint8Array): KeyPair {
  if (privateKey.length !== DHLEN) throw new NoiseError('X25519 private key must be 32 bytes');
  return { privateKey, publicKey: x25519.getPublicKey(privateKey) };
}

export function generateKeyPair(): KeyPair {
  return keyPairFromPrivate(randomBytes(DHLEN));
}

function dh(privateKey: Uint8Array, publicKey: Uint8Array): Uint8Array {
  try {
    return x25519.getSharedSecret(privateKey, publicKey);
  } catch (e) {
    throw new NoiseError(`bad public key: ${String(e)}`);
  }
}

/** HKDF from the Noise spec (section 4.3) with HMAC-SHA256, two outputs. */
function hkdf2(ck: Uint8Array, ikm: Uint8Array): [Uint8Array, Uint8Array] {
  const temp = hmac(sha256, ck, ikm);
  const out1 = hmac(sha256, temp, Uint8Array.of(1));
  const out2 = hmac(sha256, temp, concat(out1, Uint8Array.of(2)));
  return [out1, out2];
}

export class CipherState {
  private k: Uint8Array | null;
  /** Next nonce. Numbers are exact up to 2^53, far past any connection's life. */
  n = 0;

  constructor(k: Uint8Array | null = null) {
    this.k = k;
  }

  hasKey(): boolean {
    return this.k !== null;
  }

  /** 4 zero bytes + 8-byte little-endian counter. */
  private nonce(): Uint8Array {
    if (this.n >= Number.MAX_SAFE_INTEGER) throw new NoiseError('nonce exhausted');
    const nonce = new Uint8Array(12);
    const v = new DataView(nonce.buffer);
    v.setUint32(4, this.n >>> 0, true);
    v.setUint32(8, Math.floor(this.n / 0x100000000), true);
    return nonce;
  }

  encryptWithAd(ad: Uint8Array, plaintext: Uint8Array): Uint8Array {
    if (!this.k) return plaintext;
    const out = chacha20poly1305(this.k, this.nonce(), ad).encrypt(plaintext);
    this.n++;
    return out;
  }

  decryptWithAd(ad: Uint8Array, ciphertext: Uint8Array): Uint8Array {
    if (!this.k) return ciphertext;
    // The nonce only advances on success (Noise 5.1).
    let out: Uint8Array;
    try {
      out = chacha20poly1305(this.k, this.nonce(), ad).decrypt(ciphertext);
    } catch {
      throw new NoiseError('decrypt failed');
    }
    this.n++;
    return out;
  }

  encrypt(plaintext: Uint8Array): Uint8Array {
    return this.encryptWithAd(EMPTY, plaintext);
  }

  decrypt(ciphertext: Uint8Array): Uint8Array {
    return this.decryptWithAd(EMPTY, ciphertext);
  }
}

export class SymmetricState {
  ck: Uint8Array;
  h: Uint8Array;
  cipher = new CipherState();

  constructor(protocolName: string) {
    const name = asciiBytes(protocolName);
    if (name.length <= 32) {
      this.h = new Uint8Array(32);
      this.h.set(name);
    } else {
      this.h = sha256(name);
    }
    this.ck = this.h;
  }

  mixKey(ikm: Uint8Array): void {
    const [ck, k] = hkdf2(this.ck, ikm);
    this.ck = ck;
    this.cipher = new CipherState(k);
  }

  mixHash(data: Uint8Array): void {
    this.h = sha256(concat(this.h, data));
  }

  encryptAndHash(plaintext: Uint8Array): Uint8Array {
    const ct = this.cipher.encryptWithAd(this.h, plaintext);
    this.mixHash(ct);
    return ct;
  }

  decryptAndHash(ciphertext: Uint8Array): Uint8Array {
    const pt = this.cipher.decryptWithAd(this.h, ciphertext);
    this.mixHash(ciphertext);
    return pt;
  }

  split(): [CipherState, CipherState] {
    const [k1, k2] = hkdf2(this.ck, EMPTY);
    return [new CipherState(k1), new CipherState(k2)];
  }
}

export interface TransportCiphers {
  send: CipherState;
  recv: CipherState;
  /** Final handshake hash (channel binding; used by tests). */
  handshakeHash: Uint8Array;
}

/**
 * NK initiator: `<- s`, `...`, `-> e, es`, `<- e, ee`.
 * `ephemeral` is for test vectors only; leave it out in real use.
 */
export class NoiseInitiator {
  private readonly ss = new SymmetricState(NOISE_PROTOCOL);
  private readonly e: KeyPair;
  private step = 0;

  constructor(
    private readonly rs: Uint8Array,
    opts: { prologue?: Uint8Array; ephemeral?: Uint8Array } = {},
  ) {
    if (rs.length !== DHLEN) throw new NoiseError('server public key must be 32 bytes');
    this.e = opts.ephemeral ? keyPairFromPrivate(opts.ephemeral) : generateKeyPair();
    this.ss.mixHash(opts.prologue ?? NOISE_PROLOGUE);
    this.ss.mixHash(rs);
  }

  /** Message 1: `e, es` + payload. */
  writeMessage1(payload: Uint8Array = EMPTY): Uint8Array {
    if (this.step !== 0) throw new NoiseError('handshake out of order');
    this.step = 1;
    this.ss.mixHash(this.e.publicKey);
    this.ss.mixKey(dh(this.e.privateKey, this.rs));
    return concat(this.e.publicKey, this.ss.encryptAndHash(payload));
  }

  /** Message 2: `e, ee` + payload. Returns the payload and the transport ciphers. */
  readMessage2(message: Uint8Array): { payload: Uint8Array; ciphers: TransportCiphers } {
    if (this.step !== 1) throw new NoiseError('handshake out of order');
    this.step = 2;
    if (message.length < HANDSHAKE_MSG_LEN) throw new NoiseError('handshake message too short');
    const re = message.slice(0, DHLEN);
    this.ss.mixHash(re);
    this.ss.mixKey(dh(this.e.privateKey, re));
    const payload = this.ss.decryptAndHash(message.subarray(DHLEN));
    const [c1, c2] = this.ss.split();
    return { payload, ciphers: { send: c1, recv: c2, handshakeHash: this.ss.h } };
  }
}

/** NK responder (mock gateway and tests). */
export class NoiseResponder {
  private readonly ss = new SymmetricState(NOISE_PROTOCOL);
  private readonly e: KeyPair;
  private re: Uint8Array | null = null;
  private step = 0;

  constructor(
    private readonly s: KeyPair,
    opts: { prologue?: Uint8Array; ephemeral?: Uint8Array } = {},
  ) {
    this.e = opts.ephemeral ? keyPairFromPrivate(opts.ephemeral) : generateKeyPair();
    this.ss.mixHash(opts.prologue ?? NOISE_PROLOGUE);
    this.ss.mixHash(s.publicKey);
  }

  readMessage1(message: Uint8Array): Uint8Array {
    if (this.step !== 0) throw new NoiseError('handshake out of order');
    this.step = 1;
    if (message.length < HANDSHAKE_MSG_LEN) throw new NoiseError('handshake message too short');
    const re = message.slice(0, DHLEN);
    this.re = re;
    this.ss.mixHash(re);
    this.ss.mixKey(dh(this.s.privateKey, re));
    return this.ss.decryptAndHash(message.subarray(DHLEN));
  }

  writeMessage2(payload: Uint8Array = EMPTY): { message: Uint8Array; ciphers: TransportCiphers } {
    if (this.step !== 1 || !this.re) throw new NoiseError('handshake out of order');
    this.step = 2;
    this.ss.mixHash(this.e.publicKey);
    this.ss.mixKey(dh(this.e.privateKey, this.re));
    const message = concat(this.e.publicKey, this.ss.encryptAndHash(payload));
    const [c1, c2] = this.ss.split();
    return { message, ciphers: { send: c2, recv: c1, handshakeHash: this.ss.h } };
  }
}

// --- mobile.v2 framing: flag byte + chunk per Noise transport message ---

export const FLAG_LAST = 0x00;
export const FLAG_MORE = 0x01;

/** Why a frame was refused, with the WebSocket close code to use. */
export class FrameError extends Error {
  constructor(
    readonly closeCode: number,
    message: string,
  ) {
    super(message);
  }
}

type Utf8Encoder = (s: string) => Uint8Array;
type Utf8Decoder = (b: Uint8Array) => string;

const encodeUtf8: Utf8Encoder =
  typeof TextEncoder !== 'undefined'
    ? (
        (enc) => (s: string) =>
          enc.encode(s)
      )(new TextEncoder())
    : (s: string) => utf8Encode(s);
const decodeUtf8: Utf8Decoder =
  typeof TextDecoder !== 'undefined'
    ? (
        (dec) => (b: Uint8Array) =>
          dec.decode(b)
      )(new TextDecoder())
    : (b: Uint8Array) => utf8Decode(b);

/** Encrypt one JSON message into one or more transport frames. */
export function sealMessage(send: CipherState, text: string): Uint8Array[] {
  return sealBytes(send, encodeUtf8(text));
}

export function sealBytes(send: CipherState, bytes: Uint8Array): Uint8Array[] {
  const frames: Uint8Array[] = [];
  let off = 0;
  do {
    const end = Math.min(bytes.length, off + MAX_CHUNK);
    const plain = new Uint8Array(1 + end - off);
    plain[0] = end < bytes.length ? FLAG_MORE : FLAG_LAST;
    plain.set(bytes.subarray(off, end), 1);
    frames.push(send.encrypt(plain));
    off = end;
  } while (off < bytes.length);
  return frames;
}

/**
 * Decrypts transport frames and reassembles JSON messages, enforcing a size
 * cap while reassembling. `push` returns the message text when a frame
 * completes one, otherwise null; it throws FrameError on any violation.
 */
export class MessageReader {
  private chunks: Uint8Array[] = [];
  private total = 0;
  /** UTF-8 size of the last completed message. */
  lastSize = 0;

  constructor(
    private readonly recv: CipherState,
    private readonly maxBytes: number,
  ) {}

  push(frame: Uint8Array): string | null {
    if (frame.length > NOISE_MAX_MESSAGE) throw new FrameError(1009, 'frame over 65535 bytes');
    let plain: Uint8Array;
    try {
      plain = this.recv.decrypt(frame);
    } catch {
      throw new FrameError(4000, 'decrypt failed');
    }
    if (plain.length < 1) throw new FrameError(4000, 'empty frame');
    const flag = plain[0];
    if (flag !== FLAG_LAST && flag !== FLAG_MORE) throw new FrameError(4000, `unknown flag ${flag}`);
    const chunk = plain.subarray(1);
    this.total += chunk.length;
    if (this.total > this.maxBytes) throw new FrameError(1009, `message over ${this.maxBytes} bytes`);
    if (flag === FLAG_MORE) {
      this.chunks.push(chunk);
      return null;
    }
    let bytes: Uint8Array;
    if (this.chunks.length === 0) bytes = chunk;
    else {
      this.chunks.push(chunk);
      bytes = concat(...this.chunks);
    }
    this.lastSize = this.total;
    this.chunks = [];
    this.total = 0;
    return decodeUtf8(bytes);
  }
}
