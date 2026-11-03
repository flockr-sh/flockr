import { join } from 'node:path';

/**
 * Fixtures shared with the flockr server's Rust tests (`tests/fixtures/` at
 * the repo root). Edit them there; both sides read the same files.
 */
export const SHARED_FIXTURES = join(__dirname, '../../tests/fixtures');
