// Reads a blocked agent's permission prompt from the bottom of its screen:
// the question, the command or file it is about, and the numbered options.
// Claude Code and Codex both draw a list like "❯ 1. Yes" / "2. No" and pick an
// option when its digit is typed. Returns null whenever the screen does not
// clearly show one, so the app falls back to opening the terminal.
import { sha256 } from '@noble/hashes/sha2.js';
import { bytesToHex, utf8ToBytes } from '@noble/hashes/utils.js';

export interface PromptOption {
  /** The digit that picks this option. */
  key: string;
  label: string;
}

export interface Prompt {
  question: string;
  /** The command or file the prompt is about, when the screen makes it obvious. */
  detail?: string;
  options: PromptOption[];
  /** Key of the option the cursor is on. */
  selected?: string;
}

const OPTION = /^(\s*)(?:([❯›>▶●])\s*)?(\d)\.\s+(.+)$/;
// Box sides and corners agents draw around a prompt.
const BOX_EDGE = /[│┃║╭╮╰╯┌┐└┘]/g;
const SOLID_RULE = /^[─━═]{8,}$/;
const DASHED_RULE = /^[╌┄┈╍┅┉-]{8,}$/;
// Key hints at the end of an option: "(esc)", "(y)", "(shift+tab)".
const KEY_HINT = /\s+\((?:esc|enter|tab|shift\+tab|ctrl\+\w|[a-z0-9])\)$/i;

// Lines allowed below the options (key hints such as "Esc to cancel").
const MAX_LINES_BELOW = 3;
// How far above the options the question may sit.
const MAX_QUESTION_GAP = 8;
const MAX_DETAIL = 300;

interface Line {
  text: string;
  indent: number;
}

function clean(raw: string): Line {
  const text = raw.replace(BOX_EDGE, ' ').replace(/\s+$/, '');
  const indent = text.length - text.trimStart().length;
  return { text: text.trim(), indent };
}

function isRule(text: string, re: RegExp): boolean {
  return re.test(text.replace(/\s/g, ''));
}

function optionLabel(text: string): string {
  return text.replace(/\s+/g, ' ').trim().replace(KEY_HINT, '');
}

export function parsePrompt(screenText: string): Prompt | null {
  const lines = screenText.split(/\r?\n/).map(clean);
  let end = lines.length - 1;
  while (end >= 0 && !lines[end].text) end--;

  // The last option line, with no more than a few hint lines under it.
  let last = -1;
  let below = 0;
  for (let i = end; i >= 0; i--) {
    if (OPTION.test(lines[i].text ? ' '.repeat(lines[i].indent) + lines[i].text : '')) {
      last = i;
      break;
    }
    if (lines[i].text) below++;
  }
  if (last < 0) return null;

  // Walk up from there: option lines, and wrapped lines indented past the
  // option number, until anything else.
  type Found = { index: number; number: number; marker?: string; label: string; col: number };
  const found: Found[] = [];
  const pending: string[] = [];
  let first = last;
  for (let i = last; i >= 0; i--) {
    const { text, indent } = lines[i];
    if (!text) break;
    const m = OPTION.exec(' '.repeat(indent) + text);
    if (m) {
      const col = m[1].length;
      const label = [m[4], ...pending.reverse()].join(' ');
      pending.length = 0;
      found.unshift({ index: i, number: Number(m[3]), marker: m[2], label, col });
      first = i;
      continue;
    }
    const next = found[0];
    if (next && indent > next.col + 1) {
      pending.push(text);
      continue;
    }
    break;
  }
  // Indented lines above option 1 are not part of the list (pending is dropped).
  // Lines after the last option that are more indented belong to it.
  const tail: string[] = [];
  let after = last + 1;
  const lastOpt = found[found.length - 1];
  while (after <= end && lines[after].text && lines[after].indent > lastOpt.col + 1) {
    tail.push(lines[after].text);
    after++;
  }
  if (tail.length) lastOpt.label = [lastOpt.label, ...tail].join(' ');
  below = lines.slice(after, end + 1).filter((l) => l.text).length;
  if (below > MAX_LINES_BELOW) return null;

  if (found.length < 2 || found.length > 9) return null;
  if (found.some((f, i) => f.number !== i + 1)) return null;
  // The cursor marks exactly one option; a plain numbered list has none.
  const marked = found.filter((f) => f.marker);
  if (marked.length !== 1) return null;

  // The question: the nearest line above that ends with "?".
  let q = -1;
  for (let i = first - 1; i >= 0 && i >= first - MAX_QUESTION_GAP; i--) {
    const t = lines[i].text;
    if (isRule(t, SOLID_RULE)) break;
    if (t.endsWith('?')) {
      q = i;
      break;
    }
  }
  if (q < 0) return null;
  const question = lines[q].text.replace(/^[❯›>▶●✗✓⚠]\s*/, '');

  const options = found.map((f) => ({ key: String(f.number), label: optionLabel(f.label) }));
  if (options.some((o) => !o.label)) return null;
  const detail = findDetail(lines, q, first);
  return {
    question,
    ...(detail ? { detail } : {}),
    options,
    selected: String(marked[0].number),
  };
}

/** The command or file the prompt is about, or undefined when unclear. */
function findDetail(lines: Line[], q: number, first: number): string | undefined {
  // The prompt starts below the last solid rule (or box top) above the question.
  let top = q - 1;
  while (top >= 0 && q - top <= 20 && !isRule(lines[top].text, SOLID_RULE)) top--;
  const start = top >= 0 && q - top <= 20 ? top + 1 : Math.max(0, q - 6);
  const region = lines.slice(start, first).map((l) => l.text);
  const short = (s: string) => (s.length > MAX_DETAIL ? `${s.slice(0, MAX_DETAIL - 1)}…` : s);

  // A shell command shown as "$ cmd" (Codex).
  for (const t of region) {
    const m = /^\$\s+(.+)$/.exec(t);
    if (m) return short(m[1].trim());
  }
  const above = lines.slice(start, q).map((l) => l.text);
  const filled = above.filter((t) => t);
  const title = filled[0] ?? '';
  // A command between two dashed rules (Claude Code's Bash prompt).
  const dashes = above.flatMap((t, i) => (isRule(t, DASHED_RULE) ? [i] : []));
  if (/command/i.test(title) && dashes.length >= 2) {
    const inner = above.slice(dashes[0] + 1, dashes[1]).filter((t) => t);
    if (inner.length >= 1 && inner.length <= 2) return short(inner.join(' '));
  }
  // Otherwise the line under the prompt's title ("Edit file" / path).
  const second = filled[1];
  if (second && !isRule(second, DASHED_RULE) && !/^tip:/i.test(second) && !second.endsWith('?')) return short(second);
  return undefined;
}

/** Same prompt, same options: safe to answer what the person saw. */
export function samePrompt(a: Prompt, b: Prompt): boolean {
  return a.question === b.question && a.detail === b.detail;
}

/**
 * A hash of what the prompt says. A push sent with `alert_prompt_text` off
 * carries this instead of the text; the server computes it the same way
 * (`prompt_fingerprint` in `src/server/mobile/prompt.rs`).
 *
 * SHA-256 of the UTF-8 lines `q:<question>`, `d:<detail, or empty>`, then
 * `<key>:<label>` for each option in screen order, joined with "\n" (no
 * trailing newline); the first 16 bytes as lowercase hex. The cursor position
 * is left out so moving it does not block an answer.
 */
export function promptFingerprint(p: Prompt): string {
  const lines = [`q:${p.question}`, `d:${p.detail ?? ''}`, ...p.options.map((o) => `${o.key}:${o.label}`)];
  return bytesToHex(sha256(utf8ToBytes(lines.join('\n'))).slice(0, 16));
}

/** An option's label short enough for a button. */
export function shortLabel(label: string, max = 40): string {
  let s = label.split(/ · | — /)[0].trim();
  if (s.length <= max) return s;
  s = s.slice(0, max);
  const space = s.lastIndexOf(' ');
  if (space > max / 2) s = s.slice(0, space);
  return `${s.replace(/[,;:]$/, '')}…`;
}

/** The option that declines ("No", "No, and tell Claude…"). */
export function isNoOption(label: string): boolean {
  return /^no\b/i.test(label);
}
