import { diffInput } from '../../src/terminal/directInput';

describe('diffInput', () => {
  it('sends typed characters', () => {
    expect(diffInput('', 'h')).toEqual({ backspaces: 0, text: 'h' });
    expect(diffInput('he', 'hel')).toEqual({ backspaces: 0, text: 'l' });
  });

  it('sends a deletion as backspaces', () => {
    expect(diffInput('hel', 'he')).toEqual({ backspaces: 1, text: '' });
  });

  it('rewrites an autocorrected word from the first changed character', () => {
    expect(diffInput('fix teh', 'fix the ')).toEqual({ backspaces: 2, text: 'he ' });
  });

  it('counts an emoji as one backspace', () => {
    expect(diffInput('ok 👍', 'ok ')).toEqual({ backspaces: 1, text: '' });
  });

  it('does nothing for no change', () => {
    expect(diffInput('same', 'same')).toEqual({ backspaces: 0, text: '' });
  });
});
