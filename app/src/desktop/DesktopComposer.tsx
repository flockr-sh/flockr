// The desktop message box: a chat-style field that sends a message to the
// agent (Enter sends, Shift+Enter adds a line), and a hidden field that
// takes keystrokes for the terminal once it is clicked.
import React, { useImperativeHandle, useRef, useState } from 'react';
import { Pressable, StyleSheet, Text, TextInput, View } from 'react-native';
import { colors } from '../state/ui';
import { diffInput } from '../terminal/directInput';
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
};

interface Props {
  target: string;
  onSubmit: (text: string) => void;
  onPaste: (text: string) => void;
  onText: (text: string) => void;
  onKeys: (keys: string[]) => void;
  onAttach?: () => void;
  disabled?: boolean;
  ref?: React.Ref<ComposerHandle>;
}

interface KeyEvent {
  nativeEvent: { key: string; shiftKey?: boolean; ctrlKey?: boolean; metaKey?: boolean; altKey?: boolean };
  preventDefault?: () => void;
}

export function DesktopComposer({ target, onSubmit, onPaste, onText, onKeys, onAttach, disabled, ref }: Props) {
  const [text, setText] = useState('');
  const [height, setHeight] = useState(24);
  const [typing, setTyping] = useState(false);
  const typed = useRef('');
  const direct = useRef<React.ComponentRef<typeof TextInput>>(null);
  const message = useRef<React.ComponentRef<typeof TextInput>>(null);

  useImperativeHandle(ref, () => ({
    tapTerminal() {
      direct.current?.focus();
    },
  }));

  const send = (withEnter: boolean) => {
    if (!text.trim()) return;
    if (withEnter) onSubmit(text);
    else onPaste(text);
    setText('');
    setHeight(24);
  };

  const onMessageKey = (e: KeyEvent) => {
    if (e.nativeEvent.key === 'Enter' && !e.nativeEvent.shiftKey) {
      e.preventDefault?.();
      send(true);
    }
  };

  const onDirectChange = (next: string) => {
    const edit = diffInput(typed.current, next);
    if (edit.backspaces > 0) onKeys(Array(edit.backspaces).fill('backspace'));
    if (edit.text) onText(edit.text);
    typed.current = next;
  };

  const clearDirect = () => {
    typed.current = '';
    direct.current?.clear();
  };

  const onDirectKey = (e: KeyEvent) => {
    const { key, ctrlKey, shiftKey } = e.nativeEvent;
    const named = KEY_NAMES[key];
    if (ctrlKey && key.length === 1) {
      e.preventDefault?.();
      clearDirect();
      onKeys([`ctrl+${key.toLowerCase()}`]);
    } else if (named) {
      e.preventDefault?.();
      clearDirect();
      onKeys([named === 'tab' && shiftKey ? 'shift+tab' : named]);
    } else if (key === 'Enter') {
      e.preventDefault?.();
      clearDirect();
      onKeys(['enter']);
    } else if (key === 'Backspace' && typed.current === '') {
      onKeys(['backspace']);
    }
  };

  return (
    <View style={styles.dock}>
      <TextInput
        ref={direct}
        style={styles.hidden}
        defaultValue=""
        onChangeText={onDirectChange}
        onKeyPress={onDirectKey}
        onFocus={() => setTyping(true)}
        onBlur={() => {
          clearDirect();
          setTyping(false);
        }}
        autoCapitalize="none"
        autoCorrect={false}
        editable={!disabled}
      />
      <View style={[styles.box, typing && styles.boxIdle]}>
        {onAttach ? <Chip label="＋" hint="Attach an image" onPress={onAttach} disabled={disabled} /> : null}
        <TextInput
          ref={message}
          style={[styles.input, { height: Math.min(200, Math.max(24, height)) }]}
          value={text}
          onChangeText={setText}
          onKeyPress={onMessageKey}
          onContentSizeChange={(e) => setHeight(e.nativeEvent.contentSize.height)}
          placeholder={typing ? 'Typing into the terminal. Click here to write a message.' : `Message ${target}`}
          placeholderTextColor={colors.dim}
          multiline
          editable={!disabled}
        />
        <Pressable
          accessibilityRole="button"
          accessibilityLabel="Send"
          disabled={disabled || !text.trim()}
          onPress={() => send(true)}
          onLongPress={() => send(false)}
          style={[styles.send, (disabled || !text.trim()) && styles.sendOff]}
        >
          <Text style={styles.sendText}>↑</Text>
        </Pressable>
      </View>
      <Text style={styles.footnote}>
        {typing
          ? 'Keys go straight to the terminal.'
          : 'Enter sends, Shift+Enter adds a line. Click the terminal to type into it.'}
      </Text>
    </View>
  );
}

function Chip({
  label,
  hint,
  onPress,
  disabled,
}: {
  label: string;
  hint: string;
  onPress: () => void;
  disabled?: boolean;
}) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={hint}
      disabled={disabled}
      onPress={onPress}
      style={(state) => [styles.chip, (state as { hovered?: boolean }).hovered && styles.chipHover]}
    >
      <Text style={styles.chipText}>{label}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  dock: { paddingHorizontal: 24, paddingTop: 10, paddingBottom: 14, alignItems: 'center' },
  box: {
    width: '100%',
    maxWidth: 920,
    backgroundColor: '#1a1c24',
    borderRadius: 16,
    borderWidth: 1,
    borderColor: '#2c303d',
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: 10,
    paddingLeft: 16,
    paddingRight: 8,
    paddingVertical: 8,
    shadowColor: '#000',
    shadowOpacity: 0.35,
    shadowRadius: 18,
    shadowOffset: { width: 0, height: 6 },
  },
  boxIdle: { opacity: 0.7 },
  input: {
    flex: 1,
    marginBottom: 5,
    color: colors.text,
    fontSize: 15,
    lineHeight: 22,
    outlineStyle: 'none',
  } as object,
  chip: {
    minWidth: 30,
    paddingHorizontal: 8,
    paddingVertical: 4,
    borderRadius: 7,
    alignItems: 'center',
    borderWidth: 1,
    borderColor: '#2c303d',
  },
  chipHover: { backgroundColor: '#242733' },
  chipText: { color: colors.dim, fontSize: 12, fontWeight: '600' },
  send: {
    width: 34,
    height: 34,
    borderRadius: 17,
    backgroundColor: colors.text,
    alignItems: 'center',
    justifyContent: 'center',
  },
  sendOff: { backgroundColor: '#3a3e4c' },
  sendText: { color: '#0b0d12', fontSize: 17, fontWeight: '700' },
  footnote: { color: '#5d6375', fontSize: 11.5, marginTop: 8 },
  hidden: { position: 'absolute', width: 1, height: 1, opacity: 0, left: -10 },
});
