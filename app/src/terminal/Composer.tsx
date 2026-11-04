// Control-key row plus input: type mode sends keystrokes straight to the terminal,
// compose mode is a native text field (dictation, autocorrect) sent with Send.
import React, { useEffect, useImperativeHandle, useRef, useState } from 'react';
import { Keyboard, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { colors, ui } from '../state/ui';
import { diffInput } from './directInput';

export interface KeyDef {
  label: string;
  keys: string[];
}

export const KEY_ROW: KeyDef[] = [
  { label: 'Esc', keys: ['esc'] },
  { label: 'Tab', keys: ['tab'] },
  { label: '⇧Tab', keys: ['shift+tab'] },
  { label: '^C', keys: ['ctrl+c'] },
  { label: '^D', keys: ['ctrl+d'] },
  { label: '←', keys: ['left'] },
  { label: '↑', keys: ['up'] },
  { label: '↓', keys: ['down'] },
  { label: '→', keys: ['right'] },
  { label: '⌫', keys: ['backspace'] },
  { label: '⏎', keys: ['enter'] },
  { label: 'y', keys: ['y'] },
  { label: 'n', keys: ['n'] },
  { label: '1', keys: ['1'] },
  { label: '2', keys: ['2'] },
  { label: '3', keys: ['3'] },
];

export type ComposerMode = 'type' | 'compose';

// Kept for the app session, so every terminal opens in the last mode used.
let lastMode: ComposerMode = 'type';

export interface ComposerHandle {
  /** A tap on the terminal: closes the keyboard, or opens it in type mode. */
  tapTerminal(): void;
}

interface Props {
  /** Send text then Enter (`input.submit`). */
  onSubmit: (text: string) => void;
  /** Send text without Enter (`input.paste`). */
  onPaste: (text: string) => void;
  /** Send typed text as keystrokes (`input.text`). */
  onText: (text: string) => void;
  onKeys: (keys: string[]) => void;
  disabled?: boolean;
  /** Space under the composer for the home indicator; 0 while the keyboard is up. */
  bottomInset?: number;
  keyboardOpen?: boolean;
  /** Attach an image; the button is hidden when unset. */
  onAttach?: () => void;
  ref?: React.Ref<ComposerHandle>;
}

export function Composer({
  onSubmit,
  onPaste,
  onText,
  onKeys,
  disabled,
  bottomInset = 0,
  keyboardOpen,
  onAttach,
  ref,
}: Props) {
  const [mode, setModeState] = useState<ComposerMode>(lastMode);
  const [text, setText] = useState('');
  // A multiline input on iOS keeps its grown height after it is cleared, so each send
  // mounts a fresh one, focused again if the old one had the keyboard.
  const [generation, setGeneration] = useState(0);
  const focused = useRef(false);
  const [refocus, setRefocus] = useState(false);
  // Type mode: what was typed since the last Enter (see directInput.ts).
  // A ref, not state: two changes can arrive before a re-render.
  const typed = useRef('');
  const typeInput = useRef<React.ComponentRef<typeof TextInput>>(null);
  const resetTyped = () => {
    typed.current = '';
    typeInput.current?.clear();
  };
  const focusTypeInput = useRef(false);

  useImperativeHandle(ref, () => ({
    tapTerminal() {
      if (keyboardOpen) Keyboard.dismiss();
      else if (mode === 'type') typeInput.current?.focus();
    },
  }));

  useEffect(() => {
    if (mode === 'type' && focusTypeInput.current) typeInput.current?.focus();
    focusTypeInput.current = false;
  }, [mode]);

  const setMode = (next: ComposerMode) => {
    lastMode = next;
    resetTyped();
    if (next === 'type') focusTypeInput.current = !!keyboardOpen;
    else {
      setRefocus(!!keyboardOpen);
      setGeneration((g) => g + 1);
    }
    setModeState(next);
  };

  const send = (withEnter: boolean) => {
    if (!text) {
      if (withEnter) onKeys(['enter']);
      return;
    }
    if (withEnter) onSubmit(text);
    else onPaste(text);
    setText('');
    setRefocus(focused.current);
    setGeneration((g) => g + 1);
  };

  const sendKeys = (keys: string[]) => {
    // Arrows and the like move the terminal cursor away from the typed text.
    resetTyped();
    onKeys(keys);
  };

  const onTypedChange = (next: string) => {
    const edit = diffInput(typed.current, next);
    if (edit.backspaces > 0) onKeys(Array(edit.backspaces).fill('backspace'));
    if (edit.text) onText(edit.text);
    typed.current = next;
  };

  const key = (label: string, onPress: () => void, accessibilityLabel?: string) => (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={accessibilityLabel}
      key={label}
      disabled={disabled}
      onPress={onPress}
      style={({ pressed }) => [styles.key, pressed && styles.keyPressed]}
    >
      <Text style={styles.keyText}>{label}</Text>
    </Pressable>
  );

  return (
    <View style={[styles.wrap, { paddingBottom: (mode === 'type' ? 0 : 8) + bottomInset }]}>
      <ScrollView
        horizontal
        keyboardShouldPersistTaps="always"
        showsHorizontalScrollIndicator={false}
        contentContainerStyle={styles.keys}
      >
        {keyboardOpen
          ? key('⌄', () => Keyboard.dismiss(), 'Hide keyboard')
          : mode === 'type'
            ? key('⌨', () => typeInput.current?.focus(), 'Show keyboard')
            : null}
        {mode === 'type'
          ? key('✎', () => setMode('compose'), 'Compose a message')
          : key('Type', () => setMode('type'), 'Type into the terminal')}
        {mode === 'type' && onAttach ? key('＋', onAttach, 'Attach image') : null}
        {KEY_ROW.map((k) => key(k.label, () => sendKeys(k.keys)))}
      </ScrollView>
      {mode === 'type' ? (
        // Off screen: the terminal shows what is typed.
        <TextInput
          ref={typeInput}
          style={styles.hidden}
          defaultValue=""
          onChangeText={onTypedChange}
          // Backspace with nothing typed since Enter fires no change.
          onKeyPress={(e) => {
            if (e.nativeEvent.key === 'Backspace' && typed.current === '') onKeys(['backspace']);
          }}
          onSubmitEditing={() => {
            resetTyped();
            onKeys(['enter']);
          }}
          submitBehavior="submit"
          autoCapitalize="none"
          autoCorrect
          caretHidden
          editable={!disabled}
        />
      ) : (
        <View style={styles.row}>
          {onAttach ? (
            <Pressable
              accessibilityRole="button"
              accessibilityLabel="Attach image"
              disabled={disabled}
              onPress={onAttach}
              style={[ui.button, styles.send]}
            >
              <Text style={ui.buttonText}>＋</Text>
            </Pressable>
          ) : null}
          <TextInput
            key={generation}
            autoFocus={refocus}
            onFocus={() => (focused.current = true)}
            onBlur={() => (focused.current = false)}
            style={[ui.input, styles.input]}
            value={text}
            onChangeText={setText}
            placeholder="Message or command"
            placeholderTextColor={colors.dim}
            multiline
            autoCapitalize="none"
            autoCorrect
            editable={!disabled}
          />
          <Pressable
            accessibilityRole="button"
            disabled={disabled}
            onPress={() => send(true)}
            onLongPress={() => send(false)}
            style={[ui.button, ui.primary, styles.send]}
          >
            <Text style={[ui.buttonText, ui.primaryText]}>Send</Text>
          </Pressable>
        </View>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  wrap: {
    backgroundColor: colors.panel,
    borderTopWidth: StyleSheet.hairlineWidth,
    borderTopColor: colors.border,
  },
  keys: { paddingHorizontal: 6, paddingVertical: 6, gap: 6 },
  key: {
    minWidth: 40,
    paddingHorizontal: 10,
    paddingVertical: 8,
    borderRadius: 6,
    backgroundColor: colors.panelHi,
    alignItems: 'center',
  },
  keyPressed: { backgroundColor: colors.border },
  keyText: { color: colors.text, fontSize: 14, fontWeight: '600' },
  row: { flexDirection: 'row', alignItems: 'flex-end', paddingHorizontal: 8, gap: 8 },
  input: { flex: 1, maxHeight: 120 },
  send: { paddingVertical: 10 },
  hidden: { position: 'absolute', width: 1, height: 1, opacity: 0, left: -10 },
});
