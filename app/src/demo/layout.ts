// Text layout for the demo terminals: styled lines that wrap to any width,
// boxes drawn at the current width, and painting into a FakeTerminal. The
// demo redraws a whole screen on every change, so a phone's fit size and a
// desktop's size both get a screen laid out for them.
import type { Style } from '../protocol/types';
import { BLANK_STYLE, FakeTerminal, splitGraphemes } from '../../mock/terminal';

/** A run of text in one style. */
export type Seg = [string, Style];

/** One logical line. `indent` is the column wrapped rows continue at. */
export interface Para {
  segs: Seg[];
  indent?: number;
}

/** One screen row, already fitted to the width. */
export type Row = Seg[];

const rgb = (n: number) => 16777216 + n;

export const S = {
  plain: BLANK_STYLE,
  bold: [-1, -1, 1] as Style,
  dim: [rgb(0x8b909c), -1, 0] as Style,
  faint: [rgb(0x5d6270), -1, 0] as Style,
  italicDim: [rgb(0x8b909c), -1, 4] as Style,
  claude: [rgb(0xd77757), -1, 0] as Style,
  claudeBold: [rgb(0xd77757), -1, 1] as Style,
  green: [rgb(0x5fb96f), -1, 0] as Style,
  greenBold: [rgb(0x5fb96f), -1, 1] as Style,
  red: [rgb(0xe8707d), -1, 0] as Style,
  yellow: [rgb(0xe0b25a), -1, 0] as Style,
  blue: [rgb(0x7aa2f7), -1, 0] as Style,
  blueBold: [rgb(0x7aa2f7), -1, 1] as Style,
  cyan: [rgb(0x56c2c9), -1, 0] as Style,
  add: [rgb(0xcfe8d2), rgb(0x1d3a25), 0] as Style,
  del: [rgb(0xf0cdd2), rgb(0x47232a), 0] as Style,
  addSign: [rgb(0x7fd38f), rgb(0x1d3a25), 0] as Style,
  delSign: [rgb(0xf08a96), rgb(0x47232a), 0] as Style,
  userBg: [rgb(0xb6bac4), -1, 0] as Style,
};

export function width(text: string): number {
  return splitGraphemes(text).length;
}

export function para(...segs: (Seg | string)[]): Para {
  return { segs: segs.map((s) => (typeof s === 'string' ? [s, S.plain] : s)) };
}

export function indented(indent: number, ...segs: (Seg | string)[]): Para {
  return { ...para(...segs), indent };
}

export const blank: Para = { segs: [] };

/**
 * Word-wrap one paragraph to `cols`. Leading spaces of the paragraph stay;
 * continuation rows start at `indent`. Words longer than a row are split.
 */
export function wrap(p: Para, cols: number): Row[] {
  const indent = Math.min(p.indent ?? 0, Math.max(0, cols - 8));
  const rows: Row[] = [];
  let row: Row = [];
  let used = 0;
  const push = (text: string, style: Style) => {
    if (!text) return;
    const last = row[row.length - 1];
    if (last && last[1] === style) last[0] += text;
    else row.push([text, style]);
    used += width(text);
  };
  const newRow = () => {
    rows.push(row);
    row = [];
    used = 0;
    if (indent) push(' '.repeat(indent), S.plain);
  };
  // Tokens: runs of spaces and runs of non-spaces, with their style.
  const tokens: Seg[] = [];
  for (const [text, style] of p.segs) {
    for (const m of text.match(/ +|[^ ]+/g) ?? []) tokens.push([m, style]);
  }
  let atStart = true;
  for (const [tok, style] of tokens) {
    const w = width(tok);
    if (tok[0] === ' ') {
      // Spaces at a wrap point are dropped; leading ones are kept.
      if (used + w > cols) {
        if (!atStart) newRow();
        continue;
      }
      if (used === indent && rows.length > 0) continue;
      push(tok, style);
      continue;
    }
    atStart = false;
    if (used + w <= cols) {
      push(tok, style);
      continue;
    }
    if (w <= cols - indent && used > indent) {
      newRow();
      push(tok, style);
      continue;
    }
    // Split a long word across rows.
    let rest = splitGraphemes(tok);
    while (rest.length) {
      if (used >= cols) newRow();
      const take = Math.max(1, cols - used);
      push(rest.slice(0, take).join(''), style);
      rest = rest.slice(take);
    }
  }
  rows.push(row);
  return rows;
}

export function wrapAll(paras: Para[], cols: number): Row[] {
  return paras.flatMap((p) => wrap(p, cols));
}

function pad(row: Row, cols: number): Row {
  const used = row.reduce((n, [t]) => n + width(t), 0);
  return used < cols ? [...row, [' '.repeat(cols - used), S.plain]] : row;
}

/** A rounded box `boxWidth` wide around wrapped paragraphs. */
export function box(paras: Para[], boxWidth: number, border: Style, title?: Seg): Row[] {
  const inner = Math.max(4, boxWidth - 4);
  const rows: Row[] = [];
  if (title) {
    const t = ` ${title[0]} `;
    rows.push([
      ['╭───', border],
      [t, title[1]],
      ['─'.repeat(Math.max(0, boxWidth - 5 - width(t))) + '╮', border],
    ]);
  } else rows.push([['╭' + '─'.repeat(boxWidth - 2) + '╮', border]]);
  for (const p of paras) {
    for (const r of wrap(p, inner)) rows.push([['│ ', border], ...pad(r, inner), [' │', border]]);
  }
  rows.push([['╰' + '─'.repeat(boxWidth - 2) + '╯', border]]);
  return rows;
}

/** A horizontal rule with an optional label: `─ Worked for 2m ───`. */
export function rule(cols: number, label?: string): Row {
  if (!label) return [['─'.repeat(cols), S.faint]];
  const text = `─ ${label} `;
  return [[text + '─'.repeat(Math.max(0, cols - width(text))), S.faint]];
}

/** Paint the last rows that fit into `term`. Returns the row the last line landed on. */
export function paint(term: FakeTerminal, rows: Row[]): number {
  term.clear();
  const shown = rows.slice(-term.rows);
  shown.forEach((row, y) => {
    let x = 0;
    for (const [text, style] of row) {
      term.put(x, y, text, style);
      x += width(text);
    }
  });
  return shown.length - 1;
}

/** Text of the rows, for history. */
export function rowText(row: Row): string {
  return row
    .map(([t]) => t)
    .join('')
    .replace(/\s+$/, '');
}
