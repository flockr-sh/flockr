import { readFileSync } from 'fs';
import { join } from 'path';
import { isNoOption, parsePrompt, promptFingerprint, shortLabel } from '../../src/protocol/prompt';

const fixture = (name: string) => readFileSync(join(__dirname, '../fixtures/prompts', name), 'utf8');

describe('promptFingerprint', () => {
  // src/server/mobile/prompt.rs asserts the same value for the same fixture,
  // so the app and server canonical forms cannot drift apart.
  it('matches the server for the claude-bash fixture', () => {
    expect(promptFingerprint(parsePrompt(fixture('claude-bash.txt'))!)).toBe('36dfc8675e9d416e241ddc54a4bb19d3');
  });

  it('covers the question, detail, and every label but not the cursor', () => {
    const p = parsePrompt(fixture('claude-bash.txt'))!;
    const fp = promptFingerprint(p);
    expect(promptFingerprint({ ...p, selected: '3' })).toBe(fp);
    expect(promptFingerprint({ ...p, question: 'Proceed?' })).not.toBe(fp);
    expect(promptFingerprint({ ...p, detail: 'rm -rf build' })).not.toBe(fp);
    expect(promptFingerprint({ ...p, options: p.options.map((o) => (o.key === '3' ? { ...o, label: 'Yes' } : o)) })).not.toBe(
      fp,
    );
    expect(promptFingerprint({ ...p, options: p.options.slice(0, 3) })).not.toBe(fp);
    const { detail: _, ...noDetail } = p;
    expect(promptFingerprint(noDetail)).toBe(promptFingerprint({ ...p, detail: '' }));
  });
});

describe('parsePrompt', () => {
  it('reads a Claude Code Bash prompt (captured from a real pane)', () => {
    expect(parsePrompt(fixture('claude-bash.txt'))).toEqual({
      question: 'Do you want to proceed?',
      detail: 'npm test 2>&1 | tail -100',
      options: [
        { key: '1', label: 'Yes' },
        { key: '2', label: 'Yes, and don’t ask again for: npm test *' },
        { key: '3', label: 'Yes, and switch to auto mode · auto mode handles these prompts for you' },
        { key: '4', label: 'No' },
      ],
      selected: '1',
    });
  });

  it('reads a boxed Claude Code Bash prompt with a wrapped option', () => {
    expect(parsePrompt(fixture('claude-bash-boxed.txt'))).toEqual({
      question: 'Do you want to proceed?',
      detail: 'npx playwright test --project=chromium',
      options: [
        { key: '1', label: 'Yes' },
        { key: '2', label: "Yes, and don't ask again for npx playwright test commands in ~/src/storefront" },
        { key: '3', label: 'Yes, allow all Bash commands during this session' },
        { key: '4', label: 'No, and tell Claude what to do differently' },
      ],
      selected: '1',
    });
  });

  it('reads a Claude Code edit prompt', () => {
    expect(parsePrompt(fixture('claude-edit.txt'))).toEqual({
      question: 'Do you want to make this edit to Cart.tsx?',
      detail: 'src/components/Cart.tsx',
      options: [
        { key: '1', label: 'Yes' },
        { key: '2', label: 'Yes, allow all edits during this session' },
        { key: '3', label: 'No, and tell Claude what to do differently' },
      ],
      selected: '1',
    });
  });

  it('reads a Codex approval prompt', () => {
    expect(parsePrompt(fixture('codex-allow.txt'))).toEqual({
      question: 'Allow command?',
      detail: 'npm test -- --run',
      options: [
        { key: '1', label: 'Yes, proceed' },
        { key: '2', label: "Yes, and don't ask again for this command" },
        { key: '3', label: 'No, and tell Codex what to do differently' },
      ],
      selected: '1',
    });
  });

  it('reads a Codex run prompt with key hints', () => {
    const p = parsePrompt(fixture('codex-run.txt'));
    expect(p?.question).toBe('Would you like to run the following command?');
    expect(p?.detail).toBe('npx prisma migrate diff --from-schema-datasource prisma/schema.prisma');
    expect(p?.options.map((o) => o.label)).toEqual([
      'Yes, proceed',
      "Yes, and don't ask again for this command",
      'No, and tell Codex what to do differently',
    ]);
  });

  it('tracks the selected option', () => {
    const text = fixture('claude-bash.txt')
      .replace(' ❯ 1. Yes', '   1. Yes')
      .replace('   2. Yes, and', ' ❯ 2. Yes, and');
    expect(parsePrompt(text)?.selected).toBe('2');
  });

  it('ignores numbered lists that are not prompts', () => {
    expect(parsePrompt(fixture('list-idle.txt'))).toBeNull();
    expect(parsePrompt(fixture('list-question.txt'))).toBeNull();
    expect(parsePrompt(fixture('working.txt'))).toBeNull();
    expect(parsePrompt('')).toBeNull();
  });

  it('needs a question line', () => {
    const text = fixture('claude-bash.txt').replace('Do you want to proceed?', 'Pick one');
    expect(parsePrompt(text)).toBeNull();
  });

  it('needs options numbered from 1 with no gaps', () => {
    expect(parsePrompt(fixture('claude-bash.txt').replace('   2. Yes,', '   5. Yes,'))).toBeNull();
    expect(parsePrompt('Continue?\n❯ 1. Yes')).toBeNull();
  });

  it('needs the options at the bottom of the screen', () => {
    const text = `${fixture('claude-bash.txt')}\nmore\noutput\nafter\nthe\nprompt\n`;
    expect(parsePrompt(text)).toBeNull();
  });
});

describe('button labels', () => {
  it('shortens long options', () => {
    expect(shortLabel('Yes')).toBe('Yes');
    expect(shortLabel('Yes, and switch to auto mode · auto mode handles these prompts for you')).toBe(
      'Yes, and switch to auto mode',
    );
    expect(shortLabel("Yes, and don't ask again for npx playwright test commands in ~/src/storefront")).toBe(
      "Yes, and don't ask again for npx…",
    );
  });

  it('finds the No option', () => {
    expect(isNoOption('No')).toBe(true);
    expect(isNoOption('No, and tell Claude what to do differently')).toBe(true);
    expect(isNoOption('Yes, proceed')).toBe(false);
    expect(isNoOption('Nope')).toBe(false);
  });
});
