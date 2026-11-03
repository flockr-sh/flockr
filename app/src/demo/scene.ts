// The demo flock's machines, agents and what their terminals show. All text
// is made up for the demo: two projects (a storefront and a billing API)
// spread over a hub ("studio") and a second machine ("build-box").
import type { AgentStatus, Cursor, Style } from '../protocol/types';
import { FakeTerminal } from '../../mock/terminal';
import { blank, box, indented, paint, para, Para, Row, rowText, S, Seg, width, wrap, wrapAll } from './layout';

export type PaneKind = 'claude' | 'codex' | 'shell';

/** What a pane can ask of the flock around it. */
export interface PaneHost {
  now(): number;
  /** Run `fn` after `ms` of demo time. */
  later(ms: number, fn: () => void): void;
  statusChanged(pane: DemoPane): void;
}

export interface PaneDef {
  pane_id: string;
  workspace_id: string;
  tab_id: string;
  kind: PaneKind;
  /** Agent name as the sidebar shows it. */
  name?: string;
  status?: AgentStatus;
  title?: string;
  branch?: string;
  cwd: string;
  /** What the agent printed before the demo starts. */
  transcript: Para[];
  /** A shell's prompt, e.g. `dev@studio:~/src/storefront$ `. */
  shellPrompt?: string;
  /** A working agent's spinner words and the seconds it has been at it. */
  spinner?: { words: string[]; startedSecs: number; tokens: number };
  /** A permission prompt the agent waits on. */
  permission?: Permission;
}

export interface Permission {
  kind: string;
  command: string;
  why: string;
  options: string[];
  /** What runs once a person says yes. */
  onYes: (pane: DemoPane) => void;
  /** What happens on No. */
  onNo: (pane: DemoPane) => void;
}

export const DESKTOP_SIZE: [number, number] = [96, 32];

const SPIN = ['·', '✢', '✳', '✶', '✻', '✽', '✻', '✶', '✳', '✢'];

/** One demo pane: an agent or a shell, drawn into a FakeTerminal. */
export class DemoPane {
  readonly term = new FakeTerminal(...DESKTOP_SIZE);
  status: AgentStatus;
  stateChangeSeq = 1;
  transcript: Para[];
  input = '';
  permission: Permission | null;
  selected = 0;
  spinner: { words: string[]; since: number; tokens: number } | null = null;
  cursor: Cursor = { x: 0, y: 0, visible: true, shape: 0 };
  /** Every rendered row of the last draw, for history. */
  lastRows: Row[] = [];

  constructor(
    readonly def: PaneDef,
    private readonly host: PaneHost,
  ) {
    this.status = def.status ?? 'idle';
    this.transcript = [...def.transcript];
    this.permission = def.permission ?? null;
    if (def.spinner)
      this.spinner = {
        words: def.spinner.words,
        since: host.now() - def.spinner.startedSecs * 1000,
        tokens: def.spinner.tokens,
      };
    this.draw();
  }

  get id(): string {
    return this.def.pane_id;
  }

  get isAgent(): boolean {
    return this.def.kind !== 'shell';
  }

  setStatus(status: AgentStatus): void {
    if (this.status === status) return;
    this.status = status;
    this.stateChangeSeq++;
    this.host.statusChanged(this);
  }

  /** Append to the transcript and redraw. */
  say(...paras: Para[]): void {
    this.transcript.push(...paras);
    this.draw();
  }

  startSpinner(words: string[]): void {
    this.spinner = { words, since: this.host.now(), tokens: 0 };
    this.draw();
  }

  stopSpinner(): void {
    this.spinner = null;
    this.draw();
  }

  resize(cols: number, rows: number): void {
    this.term.resize(cols, rows);
    this.draw();
  }

  /** Called a few times a second; moves the spinner. */
  tick(): void {
    if (this.spinner) this.draw();
  }

  // --- input ---

  key(k: string): void {
    if (this.permission) return this.permissionKey(k);
    switch (k) {
      case 'enter':
        return this.submit();
      case 'backspace':
        this.input = [...this.input].slice(0, -1).join('');
        return this.draw();
      case 'ctrl+c':
      case 'esc':
        this.input = '';
        return this.draw();
      case 'space':
        return this.text(' ');
      default:
        if ([...k].length === 1) this.text(k);
    }
  }

  text(t: string): void {
    for (const ch of t) {
      if (ch === '\r' || ch === '\n') this.key('enter');
      else if (this.permission) this.permissionKey(ch);
      else this.input += ch;
    }
    this.draw();
  }

  private permissionKey(k: string): void {
    const p = this.permission;
    if (!p) return;
    const n = p.options.length;
    if (k === 'up') this.selected = (this.selected + n - 1) % n;
    else if (k === 'down') this.selected = (this.selected + 1) % n;
    else if (k === 'enter') return this.choose(this.selected);
    else if (k === 'esc') return this.choose(n - 1);
    else if (/^[1-9]$/.test(k) && Number(k) <= n) return this.choose(Number(k) - 1);
    this.draw();
  }

  private choose(i: number): void {
    const p = this.permission;
    if (!p) return;
    this.permission = null;
    this.selected = 0;
    if (i === p.options.length - 1) p.onNo(this);
    else p.onYes(this);
    this.draw();
  }

  /** A prompt typed into the demo: the agent answers that it is a demo. */
  private submit(): void {
    const text = this.input.trim();
    this.input = '';
    if (!text) return this.draw();
    if (this.def.kind === 'shell') {
      this.say(
        para([`${this.def.shellPrompt ?? '$ '}`, S.green], text),
        para(['demo: commands do not run on demo machines', S.dim]),
      );
      return;
    }
    const claude = this.def.kind === 'claude';
    this.say(claude ? para(['> ', S.dim], [text, S.userBg]) : para(['› ', S.bold], text), blank);
    const wasWorking = this.status === 'working';
    const hadSpinner = this.spinner;
    this.setStatus('working');
    this.startSpinner(claude ? ['Thinking…'] : ['Working']);
    this.host.later(2600, () => {
      const answer =
        'This is a demo machine, so nothing runs here. On your own flock, what you type here goes to the agent ' +
        'in this terminal, the same as typing on the computer.';
      this.say(claude ? indented(2, ['⏺ ', S.plain], answer) : indented(2, ['• ', S.dim], answer), blank);
      if (wasWorking && hadSpinner) {
        this.spinner = { ...hadSpinner };
        this.draw();
      } else {
        this.stopSpinner();
        this.setStatus('done');
      }
    });
  }

  // --- drawing ---

  draw(): void {
    const cols = this.term.cols;
    const rows = this.layout(cols);
    this.lastRows = rows.rows;
    const lastY = paint(this.term, rows.rows);
    const top = rows.rows.length - 1 - lastY;
    this.cursor = {
      x: Math.min(cols - 1, rows.cursor.x),
      y: Math.max(0, rows.cursor.y - top),
      visible: rows.cursor.visible,
      shape: this.def.kind === 'shell' ? 5 : 0,
    };
    this.term.cursorX = this.cursor.x;
    this.term.cursorY = this.cursor.y;
  }

  history(): string[] {
    return this.lastRows.map(rowText);
  }

  private layout(cols: number): { rows: Row[]; cursor: { x: number; y: number; visible: boolean } } {
    if (this.def.kind === 'shell') return this.layoutShell(cols);
    const rows: Row[] = [...this.header(cols), []];
    rows.push(...wrapAll(this.transcript, cols));
    const footer = this.def.kind === 'claude' ? this.claudeFooter(cols) : this.codexFooter(cols);
    const start = rows.length;
    rows.push(...footer.rows);
    return { rows, cursor: { ...footer.cursor, y: start + footer.cursor.y } };
  }

  private header(cols: number): Row[] {
    const w = Math.min(cols, 60);
    if (this.def.kind === 'claude') {
      return box(
        [
          para(['✻ ', S.claude], ['Welcome to ', S.plain], ['Claude Code', S.bold], '!'),
          blank,
          para(['  /help for help, /status for your current setup', S.dim]),
          blank,
          indented(7, ['  cwd: ', S.dim], [this.def.cwd, S.dim]),
        ],
        w,
        S.claude,
      );
    }
    return box(
      [
        para(['>_ ', S.dim], ['Codex', S.bold]),
        blank,
        indented(11, ['directory: ', S.dim], this.def.cwd),
        para(['approvals: ', S.dim], 'on request'),
      ],
      w,
      S.faint,
    );
  }

  private spinnerRow(cols: number): Row[] {
    const s = this.spinner;
    if (!s) return [];
    const secs = Math.max(0, Math.floor((this.host.now() - s.since) / 1000));
    const word = s.words[Math.floor(secs / 9) % s.words.length];
    const elapsed = secs >= 60 ? `${Math.floor(secs / 60)}m ${secs % 60}s` : `${secs}s`;
    if (this.def.kind === 'codex') {
      return wrap(indented(2, ['• ', S.cyan], [word, S.bold], [` (${elapsed} • esc to interrupt)`, S.dim]), cols);
    }
    const glyph = SPIN[Math.floor(this.host.now() / 400) % SPIN.length];
    const tokens = s.tokens + secs * 23;
    const tk = tokens >= 1000 ? `${(tokens / 1000).toFixed(1)}k` : `${tokens}`;
    return wrap(
      indented(
        2,
        [`${glyph} `, S.claude],
        [word, S.claude],
        [` (${elapsed} · ↓ ${tk} tokens · esc to interrupt)`, S.dim],
      ),
      cols,
    );
  }

  private claudeFooter(cols: number): { rows: Row[]; cursor: { x: number; y: number; visible: boolean } } {
    const p = this.permission;
    if (p) {
      const options = p.options.map((o, i) =>
        i === this.selected
          ? indented(5, ['❯ ', S.blue], [`${i + 1}. ${o}`, S.blue])
          : indented(5, '  ', [`${i + 1}. `, S.plain], o),
      );
      const rows = box(
        [
          para([p.kind, S.blueBold]),
          blank,
          indented(3, '   ', p.command),
          indented(3, '   ', [p.why, S.dim]),
          blank,
          para('Do you want to proceed?'),
          ...options,
        ],
        cols,
        S.blue,
      );
      return { rows, cursor: { x: 0, y: rows.length - 1, visible: false } };
    }
    const rows: Row[] = [];
    const spin = this.spinnerRow(cols);
    if (spin.length) rows.push(...spin, []);
    const inner = Math.max(4, cols - 4);
    const typed = wrap(para(['> ', S.plain], this.input), inner);
    const boxRows = box([para(['> ', S.plain], this.input)], cols, S.faint);
    const boxStart = rows.length;
    rows.push(...boxRows);
    rows.push([['  ? for shortcuts', S.dim]]);
    const last = typed[typed.length - 1];
    const x = 2 + last.reduce((n, [t]) => n + width(t), 0);
    return { rows, cursor: { x, y: boxStart + typed.length, visible: true } };
  }

  private codexFooter(cols: number): { rows: Row[]; cursor: { x: number; y: number; visible: boolean } } {
    const rows: Row[] = [];
    const spin = this.spinnerRow(cols);
    if (spin.length) rows.push(...spin, []);
    const start = rows.length;
    const typed = this.input
      ? wrap(para(['› ', S.bold], this.input), cols)
      : wrap(para(['› ', S.bold], ['Ask Codex to do anything', S.faint]), cols);
    rows.push(...typed, []);
    rows.push([['  ⏎ send   ⌃J newline   ⌃C quit', S.dim]]);
    const last = typed[typed.length - 1];
    const x = this.input ? last.reduce((n, [t]) => n + width(t), 0) : 2;
    return { rows, cursor: { x, y: start + typed.length - 1, visible: true } };
  }

  private layoutShell(cols: number): { rows: Row[]; cursor: { x: number; y: number; visible: boolean } } {
    const rows = wrapAll(this.transcript, cols);
    const prompt = wrap(para([this.def.shellPrompt ?? '$ ', S.green], this.input), cols);
    rows.push(...prompt);
    const last = prompt[prompt.length - 1];
    return {
      rows,
      cursor: { x: last.reduce((n, [t]) => n + width(t), 0), y: rows.length - 1, visible: true },
    };
  }
}

// --- the scene ---

const tool = (name: string, arg: string): Para => indented(2, ['⏺ ', S.green], [name, S.bold], `(${arg})`);
const say = (...segs: (Seg | string)[]): Para => indented(2, ['⏺ ', S.plain], ...segs);
const out = (text: string, style = S.dim): Para => indented(5, ['  ⎿  ', S.dim], [text, style]);
const more = (text: string, style = S.dim): Para => indented(5, '     ', [text, style]);
const user = (text: string): Para => indented(2, ['> ', S.dim], [text, S.userBg]);
const diff = (n: number, sign: ' ' | '+' | '-', code: string): Para => {
  const num = String(n).padStart(4);
  if (sign === ' ') return indented(12, ['     ', S.plain], [`${num}   ${code}`, S.dim]);
  const style = sign === '+' ? S.add : S.del;
  return indented(
    12,
    ['     ', S.plain],
    [`${num} `, style],
    [sign, sign === '+' ? S.addSign : S.delSign],
    [`  ${code}`, style],
  );
};

const cx = {
  you: (text: string): Para => indented(2, ['› ', S.bold], text),
  bullet: (...segs: (Seg | string)[]): Para => indented(2, ['• ', S.dim], ...segs),
  ran: (cmd: string): Para => indented(2, ['• ', S.green], ['Ran ', S.bold], cmd),
  tree: (text: string, style = S.dim): Para => indented(4, ['  └ ', S.dim], [text, style]),
  sub: (text: string, style = S.dim): Para => indented(4, '    ', [text, style]),
};

const SEVERITY: Record<string, Style> = { High: S.red, Medium: S.yellow, Low: S.blue };
const finding = (n: number, severity: string, text: string): Para =>
  indented(5, [`  ${n}. `, S.plain], [`${severity}: `, SEVERITY[severity]], text);

export interface MachineDef {
  id: string;
  name: string;
  host: string;
  workspaces: { workspace_id: string; label: string; tabs: { tab_id: string; label: string }[] }[];
  panes: PaneDef[];
}

/** The reply studio:api sends once a person approves the message. */
export const API_REPLY =
  'Integer cents. amount_cents, tax_cents and total_cents are integers in every /invoices response. ' +
  'Compute the 10% off on cents and round to a whole cent.';

export const COUPON_QUESTION =
  "Are invoice totals in cents (integer) or dollars (decimal)? I'm adding a 10% SAVE10 coupon to the " +
  'storefront cart and need to match the unit.';

const PLAYWRIGHT = [
  'checkout › guest checkout with card (2.1s)',
  'checkout › saved address prefills shipping (1.4s)',
  'checkout › rejects an expired card (1.2s)',
  'cart › quantity stepper updates subtotal (0.9s)',
  'cart › empty cart shows the browse link (0.6s)',
  'account › order history lists past orders (1.7s)',
];

function checkoutYes(pane: DemoPane, host: PaneHost): void {
  pane.say(tool('Bash', 'npx playwright test --project=chromium'));
  pane.setStatus('working');
  pane.startSpinner(['Running…', 'Waiting for tests…']);
  host.later(900, () => pane.say(out('Running 12 tests using 4 workers')));
  PLAYWRIGHT.forEach((line, i) => host.later(1500 + i * 450, () => pane.say(more(`✓  ${line}`, S.green))));
  const end = 1500 + PLAYWRIGHT.length * 450 + 400;
  host.later(end, () => pane.say(more('… +6 passed (ctrl+o to expand)'), more('12 passed (18.4s)', S.green), blank));
  host.later(end + 1400, () => {
    pane.stopSpinner();
    pane.say(
      say(
        'All 8 unit test files and all 12 end-to-end tests pass in Chromium. Nothing needed fixing; ' +
          'the storefront suite is green on main.',
      ),
      blank,
    );
    pane.setStatus('done');
  });
}

function checkoutNo(pane: DemoPane): void {
  pane.say(
    tool('Bash', 'npx playwright test --project=chromium'),
    out('No (tell Claude what to do differently)', S.red),
    blank,
  );
  pane.setStatus('idle');
}

export function sceneMachines(host: PaneHost): MachineDef[] {
  return [
    {
      id: 'studio',
      name: 'studio',
      host: 'studio',
      workspaces: [
        { workspace_id: 'w_store', label: 'storefront', tabs: [{ tab_id: 't_store_1', label: '1' }] },
        {
          workspace_id: 'w_billing',
          label: 'billing-api',
          tabs: [
            { tab_id: 't_billing_1', label: '1' },
            { tab_id: 't_billing_2', label: 'dev server' },
          ],
        },
        { workspace_id: 'w_infra', label: 'infra', tabs: [{ tab_id: 't_infra_1', label: '1' }] },
      ],
      panes: [
        {
          pane_id: 'p_checkout',
          workspace_id: 'w_store',
          tab_id: 't_store_1',
          kind: 'claude',
          name: 'checkout',
          status: 'blocked',
          title: 'Storefront test suite',
          branch: 'main',
          cwd: '~/src/storefront',
          transcript: [
            user('Run the storefront test suite and fix whatever fails.'),
            blank,
            say("I'll run the unit tests first, then the end-to-end suite."),
            blank,
            tool('Bash', 'npm run test:unit'),
            out('✓ src/cart/cart.test.ts (14 tests) 38ms', S.green),
            more('✓ src/checkout/address.test.ts (9 tests) 22ms', S.green),
            more('✓ src/checkout/payment.test.ts (11 tests) 41ms', S.green),
            more('… +5 lines (ctrl+o to expand)'),
            more('Test Files  8 passed (8)'),
            more('     Tests  61 passed (61)'),
            blank,
            say('Unit tests pass. Next the Playwright suite, which starts a preview server on port 4173.'),
            blank,
          ],
          permission: {
            kind: 'Bash command',
            command: 'npx playwright test --project=chromium',
            why: 'Run the end-to-end checkout tests in Chromium',
            options: [
              'Yes',
              "Yes, and don't ask again for npx playwright test commands in ~/src/storefront",
              'Yes, allow all Bash commands during this session',
              'No, and tell Claude what to do differently (esc)',
            ],
            onYes: (pane) => checkoutYes(pane, host),
            onNo: checkoutNo,
          },
        },
        {
          pane_id: 'p_api',
          workspace_id: 'w_billing',
          tab_id: 't_billing_1',
          kind: 'claude',
          name: 'api',
          status: 'working',
          title: 'Per-customer rate limiting for /invoices',
          branch: 'worktree/invoices-rate-limit',
          cwd: '~/src/billing-api',
          spinner: { words: ['Writing tests…', 'Thinking…', 'Running vitest…'], startedSecs: 94, tokens: 1200 },
          transcript: [
            user(
              'Add per-customer rate limiting to the /invoices routes: 60 requests a minute, then 429 with Retry-After.',
            ),
            blank,
            say("I'll look at how the routes and middleware are wired first."),
            blank,
            tool('Read', 'src/routes/invoices.ts'),
            out('Read 142 lines'),
            blank,
            tool('Search', 'pattern: "redis|rateLimit", path: "src"'),
            out('Found 3 files'),
            blank,
            say(
              "There's a Redis client in src/lib/redis.ts already. I'll add a fixed-window limiter keyed by " +
                'customer ID and mount it after auth.',
            ),
            blank,
            tool('Write', 'src/middleware/rateLimit.ts'),
            out('Wrote 41 lines to src/middleware/rateLimit.ts'),
            blank,
            tool('Update', 'src/routes/invoices.ts'),
            out('Updated src/routes/invoices.ts with 2 additions and 1 removal'),
            diff(1, ' ', "import { Router } from 'express'"),
            diff(2, ' ', "import { requireCustomer } from '../middleware/auth'"),
            diff(3, '+', "import { rateLimit } from '../middleware/rateLimit'"),
            diff(4, ' ', ''),
            diff(5, ' ', 'const router = Router()'),
            diff(6, '-', 'router.use(requireCustomer)'),
            diff(6, '+', 'router.use(requireCustomer, rateLimit({ limit: 60, windowSec: 60 }))'),
            blank,
            say('Now tests for the limit and the Retry-After header.'),
            blank,
          ],
        },
        {
          pane_id: 'p_infra',
          workspace_id: 'w_infra',
          tab_id: 't_infra_1',
          kind: 'codex',
          name: 'infra-review',
          status: 'done',
          title: 'Review billing Postgres risks',
          branch: 'main',
          cwd: '~/src/infra',
          transcript: [
            cx.you(
              'Review the billing Postgres setup in terraform/billing for risks before next week’s migration. ' +
                'Do not change anything.',
            ),
            blank,
            cx.bullet('Explored'),
            cx.tree('Read rds.tf, variables.tf, alarms.tf'),
            cx.sub('Search max_connections in terraform/billing'),
            blank,
            cx.bullet(['Findings', S.bold]),
            blank,
            finding(1, 'High', 'backup_retention_period = 1 in rds.tf. Point-in-time restore only reaches back one day.'),
            finding(2, 'Medium', 'deletion_protection = false on the billing instance.'),
            finding(3, 'Medium', 'max_connections = 100, but 6 api replicas with a pool of 20 open 120.'),
            finding(4, 'Low', 'no alarm on FreeStorageSpace.'),
            blank,
            indented(4, '    ', 'No files changed. I can draft the Terraform changes for 1 to 3 if you want.'),
            blank,
            { segs: [['─ Worked for 3m 02s ─────────────────────', S.faint]] },
            blank,
          ],
        },
        {
          pane_id: 'p_devserver',
          workspace_id: 'w_billing',
          tab_id: 't_billing_2',
          kind: 'shell',
          cwd: '~/src/billing-api',
          title: 'npm run dev',
          shellPrompt: 'dev@studio:~/src/billing-api$ ',
          transcript: [
            para(['dev@studio:~/src/billing-api$ ', S.green], 'npm run dev'),
            blank,
            para('> billing-api@3.8.1 dev'),
            para('> tsx watch src/server.ts'),
            blank,
            para(['[db] ', S.cyan], 'connected to postgres://localhost:5432/billing'),
            para(['[redis] ', S.cyan], 'connected to redis://localhost:6379'),
            para(['[http] ', S.green], 'listening on http://localhost:3001'),
            para(['[http] ', S.green], 'GET /invoices?customer=cus_demo42 200 18ms'),
            para(['[http] ', S.green], 'GET /invoices/inv_1007 200 6ms'),
            para(['^C', S.dim]),
          ],
        },
      ],
    },
    {
      id: 'm_buildbox',
      name: 'build-box',
      host: 'build-box',
      workspaces: [
        { workspace_id: 'w_billing', label: 'billing-api', tabs: [{ tab_id: 't_billing_1', label: '1' }] },
        {
          workspace_id: 'w_store',
          label: 'storefront',
          tabs: [
            { tab_id: 't_store_1', label: '1' },
            { tab_id: 't_store_2', label: '2' },
          ],
        },
      ],
      panes: [
        {
          pane_id: 'p_tests',
          workspace_id: 'w_billing',
          tab_id: 't_billing_1',
          kind: 'codex',
          name: 'tests',
          status: 'done',
          title: 'Invoices route vitest tests',
          branch: 'worktree/invoices-tests',
          cwd: '~/src/billing-api',
          transcript: [
            cx.you('Write vitest tests for src/routes/invoices.ts. Cover list, get, create, and the 404s.'),
            blank,
            cx.bullet('I’ll read the route and the existing test setup first.'),
            blank,
            cx.bullet('Explored'),
            cx.tree('Read invoices.ts, vitest.config.ts, tests/helpers/app.ts'),
            blank,
            indented(2, ['• ', S.green], ['Edited ', S.bold], 'tests/routes/invoices.test.ts ', ['(+96 -0)', S.green]),
            diff(1, '+', "import { describe, expect, it } from 'vitest'"),
            diff(2, '+', "import { testApp, seedInvoice } from '../helpers/app'"),
            diff(3, '+', ''),
            diff(4, '+', "describe('GET /invoices', () => {"),
            diff(5, '+', "  it('lists only the customer’s invoices', async () => {"),
            more('… +91 lines'),
            blank,
            cx.ran('npx vitest run tests/routes/invoices.test.ts'),
            cx.tree('✓ tests/routes/invoices.test.ts (9 tests) 41ms', S.green),
            cx.sub('Test Files  1 passed (1)'),
            cx.sub('     Tests  9 passed (9)'),
            blank,
            { segs: [['─ Worked for 2m 14s ─────────────────────', S.faint]] },
            blank,
            cx.bullet(
              'Added 9 tests for GET, POST and GET /:id on /invoices, including 404s for an unknown invoice and ' +
                'another customer’s invoice. All pass.',
            ),
            blank,
          ],
        },
        {
          pane_id: 'p_storefront_ui',
          workspace_id: 'w_store',
          tab_id: 't_store_1',
          kind: 'claude',
          name: 'storefront-ui',
          status: 'done',
          title: 'Cart.tsx SAVE10 coupon',
          branch: 'worktree/save10-coupon',
          cwd: '~/src/storefront',
          transcript: [
            user('Add a SAVE10 coupon to the cart: 10% off the subtotal, shown as its own line.'),
            blank,
            tool('Read', 'src/components/Cart.tsx'),
            out('Read 96 lines'),
            blank,
            say('Cart totals come from the billing API. I need to know their unit before I take 10% off.'),
            blank,
            tool('Bash', 'flockr message send studio:api "Are invoice totals in cents (integer) or dollars …"'),
            out('Waiting for a person to approve it (other project)'),
            blank,
            tool('Update', 'src/components/Cart.tsx'),
            out('Updated src/components/Cart.tsx with 6 additions'),
            diff(41, ' ', 'const subtotal = sumLines(lines)'),
            diff(42, '+', "const discount = coupon === 'SAVE10' ? Math.round(subtotal * 0.1) : 0"),
            diff(43, '+', '// TODO: confirm the unit with studio:api'),
            diff(44, ' ', 'const total = subtotal - discount + shipping'),
            blank,
            say(
              'Added SAVE10 behind applyCoupon() with a discount line in the summary. The math assumes integer ' +
                'cents for now; I asked studio:api to confirm and left a TODO until it answers.',
            ),
            blank,
          ],
        },
        {
          pane_id: 'p_shell',
          workspace_id: 'w_store',
          tab_id: 't_store_2',
          kind: 'shell',
          cwd: '~/src/storefront',
          title: 'zsh',
          shellPrompt: 'dev@build-box:~/src/storefront$ ',
          transcript: [
            para(['dev@build-box:~/src/storefront$ ', S.green], 'git status --short'),
            para([' M ', S.red], 'src/components/Cart.tsx'),
            para(['?? ', S.red], 'src/lib/coupons.ts'),
          ],
        },
      ],
    },
  ];
}

/** Lines the hub's api agent prints when a person approves the message. */
export function apiGetsMessage(): Para[] {
  return [
    indented(2, ['⏺ ', S.cyan], ['Message from build-box:storefront-ui', S.bold]),
    out(COUPON_QUESTION, S.plain),
    blank,
    tool('Bash', 'flockr message reply build-box:storefront-ui "Integer cents. …"'),
    out('Delivered to build-box:storefront-ui'),
    blank,
  ];
}

/** Lines storefront-ui prints when the reply arrives. */
export function storefrontGetsReply(): Para[] {
  return [
    indented(2, ['⏺ ', S.cyan], ['Reply from studio:api', S.bold]),
    out(API_REPLY, S.plain),
    blank,
    say('That matches what Cart.tsx assumes. I removed the TODO.'),
    blank,
  ];
}

