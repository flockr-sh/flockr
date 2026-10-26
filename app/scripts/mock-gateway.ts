// Run the mock gateway so the app can be tried without a flockr server.
//   npm run mock-gateway -- --host 100.92.14.3 --port 47390
// Options:
//   --host <ip>         address to bind (default 127.0.0.1; use your NetBird/LAN IP for a phone)
//   --port <n>          port (default 47390)
//   --log-rate <n>      lines per second on the Codex log pane (default 20)
//   --stress-fps <n>    whole-screen redraws per second on the stress pane (default 30; 0 = off)
//   --no-shift          do not emit `shift` in deltas
//   --churn             flip the Codex agent between working/blocked/done every 15 s
// eslint-disable-next-line @typescript-eslint/no-require-imports
const qrcode = require('qrcode-terminal') as {
  generate: (text: string, opts: { small: boolean }, cb: (qr: string) => void) => void;
};
import { MockGateway } from '../mock/gateway';

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

async function main(): Promise<void> {
  const host = arg('--host') ?? '127.0.0.1';
  const gw = new MockGateway({
    host,
    port: Number(arg('--port') ?? 47390),
    logLinesPerSec: Number(arg('--log-rate') ?? 20),
    stressFps: Number(arg('--stress-fps') ?? 30),
    shift: !process.argv.includes('--no-shift'),
    log: (m) => console.log(`[mock] ${m}`),
  });
  const { port } = await gw.start();
  console.log(`mock flockr gateway on ws://${host}:${port}/mobile/v1`);
  const printPairing = () => {
    const url = gw.createPairing(600);
    qrcode.generate(url, { small: true }, (qr) => console.log(qr));
    console.log(url);
    console.log('Pairing code expires in 10:00. Press p + Enter for a new one.');
  };
  printPairing();
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', (d: string) => {
    if (d.trim() === 'p') printPairing();
    if (d.trim() === 'b') gw.setStatus('p_2', 'blocked');
    if (d.trim() === 'd') gw.setStatus('p_2', 'done');
    if (d.trim() === 'w') gw.setStatus('p_2', 'working');
  });
  console.log('Keys: p = new pairing code, b/d/w = set Codex blocked/done/working.');
  if (process.argv.includes('--churn')) {
    const cycle: ('working' | 'blocked' | 'done')[] = ['blocked', 'working', 'done', 'working'];
    let i = 0;
    setInterval(() => gw.setStatus('p_2', cycle[i++ % cycle.length]), 15000);
  }
  const stop = () => {
    void gw.stop().then(() => process.exit(0));
  };
  process.on('SIGINT', stop);
  process.on('SIGTERM', stop);
}

void main();
