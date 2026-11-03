// Runs before the app. Hermes (and Expo Go) has no WebCrypto, and the Noise
// handshake needs a CSPRNG for its ephemeral key: back
// globalThis.crypto.getRandomValues with expo-crypto when it is missing.
import * as ExpoCrypto from 'expo-crypto';

type RandomFill = <T extends ArrayBufferView | null>(array: T) => T;
const g = globalThis as { crypto?: { getRandomValues?: RandomFill } };

if (typeof g.crypto?.getRandomValues !== 'function') {
  const getRandomValues: RandomFill = (array) => {
    if (!array) return array;
    const bytes = new Uint8Array(array.buffer, array.byteOffset, array.byteLength);
    // expo-crypto fills typed arrays natively; do it in 64 KiB slices like WebCrypto's limit.
    for (let off = 0; off < bytes.length; off += 65536) {
      ExpoCrypto.getRandomValues(bytes.subarray(off, Math.min(bytes.length, off + 65536)));
    }
    return array;
  };
  if (g.crypto) g.crypto.getRandomValues = getRandomValues;
  else g.crypto = { getRandomValues };
}
