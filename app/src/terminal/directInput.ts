// Type mode: a hidden text field holds what was typed since the last Enter, and
// each change is replayed on the terminal as backspaces plus new text. That also
// covers autocorrect, which rewrites a word already sent.

export interface InputEdit {
  backspaces: number;
  text: string;
}

/** The edit that turns `prev` into `next`, in code points (one backspace each). */
export function diffInput(prev: string, next: string): InputEdit {
  const a = Array.from(prev);
  const b = Array.from(next);
  let same = 0;
  while (same < a.length && same < b.length && a[same] === b[same]) same++;
  return { backspaces: a.length - same, text: b.slice(same).join('') };
}
