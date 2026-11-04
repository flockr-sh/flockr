import { readFileSync } from 'node:fs';
import { join } from 'node:path';

export function fixture<T = Record<string, unknown>>(name: string): T {
  return JSON.parse(readFileSync(join(__dirname, '../../tests/fixtures/mobile-v1', `${name}.json`), 'utf8')) as T;
}
