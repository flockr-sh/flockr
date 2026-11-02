// Desktop typing: a hidden field takes the keyboard while an agent is open, so
// keys go straight to the terminal like a real one. It takes focus when the
// pane opens, when the terminal is clicked, and when a key is pressed outside
// a text field. Pasted text goes in as a paste; a pasted image is attached.
import React, { useEffect, useImperativeHandle, useRef } from 'react';
import { StyleSheet, TextInput } from 'react-native';
import { diffInput } from '../terminal/directInput';
import { pastedImage } from './pasteImage';
import type { ComposerHandle } from '../terminal/Composer';

// Browser key names the terminal takes as keys rather than text.
const KEY_NAMES: Record<string, string> = {
  ArrowUp: 'up',
  ArrowDown: 'down',
  ArrowLeft: 'left',
  ArrowRight: 'right',
  Escape: 'esc',
  Tab: 'tab',
  Home: 'home',
  End: 'end',
  PageUp: 'pageup',
  PageDown: 'pagedown',
  Delete: 'delete',
  Enter: 'enter',
};

interface Props {
  onPaste: (text: string) => void;
  onText: (text: string) => void;
  onKeys: (keys: string[]) => void;
  /** Set when the server takes images. */
  onImage?: (file: File) => void;
  disabled?: boolean;
  ref?: React.Ref<ComposerHandle>;
}

interface Key {
  key: string;
  shiftKey?: boolean;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
}

/** The terminal keys for a key press, or null to let the browser handle it. */
function keysFor({ key, ctrlKey, shiftKey, metaKey, altKey }: Key): string[] | null {
  if (metaKey) return null;
  // Ctrl+V (and Ctrl+Shift+V) pastes through the browser.
  if (ctrlKey && key.toLowerCase() === 'v') return null;
  if (ctrlKey && key.length === 1) return [`ctrl+${key.toLowerCase()}`];
  const named = KEY_NAMES[key];
  if (named) {
    if (named === 'tab' && shiftKey) return ['shift+tab'];
    return [altKey ? `alt+${named}` : named];
  }
  return null;
}

export function DesktopKeyboard({ onPaste, onText, onKeys, onImage, disabled, ref }: Props) {
  const typed = useRef('');
  const field = useRef<React.ComponentRef<typeof TextInput>>(null);

  useImperativeHandle(ref, () => ({
    tapTerminal() {
      field.current?.focus();
    },
  }));

  useEffect(() => {
    if (!disabled) field.current?.focus();
  }, [disabled]);

  const clear = () => {
    typed.current = '';
    field.current?.clear();
  };

  // A key pressed outside a text field (after a click on the sidebar or the
  // page) brings the keyboard back to the terminal.
  useEffect(() => {
    if (disabled || typeof window === 'undefined') return;
    const onKeyDown = (e: KeyboardEvent) => {
      const active = document.activeElement as HTMLElement | null;
      if (active?.isContentEditable || active?.tagName === 'INPUT' || active?.tagName === 'TEXTAREA') return;
      field.current?.focus();
      const keys = keysFor(e);
      if (keys) {
        e.preventDefault();
        onKeys(keys);
      } else if (e.key === 'Backspace') {
        e.preventDefault();
        onKeys(['backspace']);
      } else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey) {
        e.preventDefault();
        onText(e.key);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [disabled, onKeys, onText]);

  useEffect(() => {
    if (disabled || !onImage || typeof window === 'undefined') return;
    const onPasteEvent = (e: ClipboardEvent) => {
      const active = document.activeElement;
      if (active !== field.current && (active?.tagName === 'INPUT' || active?.tagName === 'TEXTAREA')) return;
      const image = pastedImage(e.clipboardData);
      if (!image) return;
      e.preventDefault();
      onImage(image);
    };
    window.addEventListener('paste', onPasteEvent);
    return () => window.removeEventListener('paste', onPasteEvent);
  }, [disabled, onImage]);

  const onChange = (next: string) => {
    const edit = diffInput(typed.current, next);
    if (edit.backspaces > 0) onKeys(Array(edit.backspaces).fill('backspace'));
    if (edit.text.length > 1) onPaste(edit.text);
    else if (edit.text) onText(edit.text);
    typed.current = next;
  };

  const onKeyPress = (e: { nativeEvent: Key; preventDefault?: () => void }) => {
    const keys = keysFor(e.nativeEvent);
    if (keys) {
      e.preventDefault?.();
      clear();
      onKeys(keys);
    } else if (e.nativeEvent.key === 'Backspace' && typed.current === '') {
      onKeys(['backspace']);
    }
  };

  return (
    <TextInput
      ref={field}
      style={styles.hidden}
      defaultValue=""
      onChangeText={onChange}
      onKeyPress={onKeyPress}
      onBlur={clear}
      autoCapitalize="none"
      autoCorrect={false}
      multiline
      editable={!disabled}
    />
  );
}

const styles = StyleSheet.create({
  hidden: { position: 'absolute', width: 1, height: 1, opacity: 0, left: -10 },
});
