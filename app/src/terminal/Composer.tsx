// Native text composer (keyboard, dictation, autocorrect) plus a control-key row.
import React, { useRef, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { colors, ui } from '../state/ui';

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

interface Props {
  /** Send text then Enter (`input.submit`). */
  onSubmit: (text: string) => void;
  /** Send text without Enter (`input.paste`). */
  onPaste: (text: string) => void;
  onKeys: (keys: string[]) => void;
  disabled?: boolean;
  /** Space under the composer for the home indicator; 0 while the keyboard is up. */
  bottomInset?: number;
}

export function Composer({ onSubmit, onPaste, onKeys, disabled, bottomInset = 0 }: Props) {
  const [text, setText] = useState('');
  // A multiline input on iOS keeps its grown height after it is cleared, so each send
  // mounts a fresh one, focused again if the old one had the keyboard.
  const [generation, setGeneration] = useState(0);
  const focused = useRef(false);
  const [refocus, setRefocus] = useState(false);
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
  return (
    <View style={[styles.wrap, { paddingBottom: 8 + bottomInset }]}>
      <ScrollView
        horizontal
        keyboardShouldPersistTaps="always"
        showsHorizontalScrollIndicator={false}
        contentContainerStyle={styles.keys}
      >
        {KEY_ROW.map((k) => (
          <Pressable
            accessibilityRole="button"
            key={k.label}
            disabled={disabled}
            onPress={() => onKeys(k.keys)}
            style={({ pressed }) => [styles.key, pressed && styles.keyPressed]}
          >
            <Text style={styles.keyText}>{k.label}</Text>
          </Pressable>
        ))}
      </ScrollView>
      <View style={styles.row}>
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
});
