// Drive the server's real Expo push path with a fake token and report what Expo said.
//
//   npx tsx scripts/push-check.ts --flockr <path/to/flockr> --pane <id> --log <flockr-server.log>
//
// Pairs a throwaway device, registers a fake Expo token, puts the app in the
// background (so the server pushes instead of sending in-band), fakes a blocked
// agent, then reads the server log for the Expo reply. Expo rejects the fake
// token, which proves the request reached Expo. The device is revoked at the end.
// Use a plain shell pane: on an agent pane, live detection overwrites the fake
// status before the server's re-check and the alert is (correctly) dropped.
// Run with the same env as the server (XDG_*, FLOCKR_SOCKET_PATH unset).
import { execFileSync } from 'node:child_process';
import { readFileSync, statSync } from 'node:fs';
import { WebSocket } from 'ws';
import { Credentials, GatewayClient, parseHostEntry, parsePairingUrl } from '../src/protocol';

const arg = (name: string) => {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
};
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function main(): Promise<void> {
  const bin = arg('--flockr');
  const pane = arg('--pane');
  const log = arg('--log');
  if (!bin || !pane || !log) throw new Error('need --flockr <bin> --pane <id> --log <file>');
  const run = (...args: string[]) => execFileSync(bin, args, { encoding: 'utf8' });

  const payload = parsePairingUrl((JSON.parse(run('mobile', 'pair', '--no-wait', '--json')) as { url: string }).url);
  const hosts = payload.hosts.map((h) => parseHostEntry(h, payload.port));
  const make = (credentials: Credentials) =>
    new GatewayClient({
      hosts,
      sid: payload.sid,
      credentials,
      app: { version: '0.1.0', platform: 'node' },
      createSocket: (url, protocols) => new WebSocket(url, protocols) as never,
      reconnect: false,
      pingIntervalMs: 0,
    });

  const pairer = make({ kind: 'pair', code: payload.code, device_name: 'push-check' });
  const creds = await new Promise<{ device_id: string; token: string }>((resolve, reject) => {
    pairer.on('paired', resolve);
    pairer.connect();
    pairer.waitReady().catch(reject);
  });
  pairer.close();

  const client = make({ kind: 'auth', device_id: creds.device_id, token: creds.token });
  client.connect();
  await client.waitReady();
  await client.registerPush('ExponentPushToken[flockr-push-check-not-real]', 'ios');
  client.setForeground(false);
  await sleep(300);

  const logStart = statSync(log).size;
  run('pane', 'report-agent', '--source', 'push-check', '--agent', 'claude', '--state', 'blocked', pane);
  // confirm_delay_ms (default 5000) plus the Expo round trip.
  let found = '';
  for (let i = 0; i < 30 && !found; i++) {
    await sleep(500);
    const tail = readFileSync(log, 'utf8').slice(logStart);
    found = tail.split('\n').find((l) => /Expo/.test(l)) ?? '';
  }
  run('pane', 'release-agent', '--source', 'push-check', '--agent', 'claude', pane);
  client.close();
  run('mobile', 'devices', 'revoke', creds.device_id);

  if (!found) {
    console.log('FAIL: no Expo reply in the server log within 15 s');
    process.exit(1);
  }
  console.log(`OK: ${found.trim()}`);
}

main().catch((e: unknown) => {
  console.error(e);
  process.exit(1);
});
