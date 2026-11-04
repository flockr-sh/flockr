// Full-screen plain-text view of terminal lines with native selection and copy.
import * as Clipboard from 'expo-clipboard';
import React, { useMemo, useRef, useState } from 'react';
import { Modal, Platform, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { SafeAreaProvider, SafeAreaView, useSafeAreaInsets } from 'react-native-safe-area-context';
import { colors, mono, ui } from '../state/ui';

interface Props {
  title: string;
  lines: string[] | null;
  onClose: () => void;
}

// Monospace advance as a fraction of the font size, with a little slack for the text view's insets.
const ADVANCE = 0.62;
const MAX_FONT = 11;
const MIN_FONT = 6;

export function TextSheet({ title, lines, onClose }: Props) {
  return (
    <Modal visible={lines !== null} animationType="slide" onRequestClose={onClose}>
      {/* A modal is a separate native root on iOS, so it needs its own provider to get insets. */}
      <SafeAreaProvider>
        <Sheet title={title} lines={lines ?? []} onClose={onClose} />
      </SafeAreaProvider>
    </Modal>
  );
}

function Sheet({ title, lines, onClose }: { title: string; lines: string[]; onClose: () => void }) {
  const insets = useSafeAreaInsets();
  const scroll = useRef<React.ComponentRef<typeof ScrollView>>(null);
  const [copied, setCopied] = useState(false);
  const [width, setWidth] = useState(0);
  const text = lines.join('\n');
  // Shrink the font so the longest line fits; wrapped terminal lines are hard to read.
  const fontSize = useMemo(() => {
    const cols = Math.max(1, ...lines.map((l) => Array.from(l).length));
    const fit = (width - 2 * PAD) / (cols * ADVANCE);
    return width ? Math.max(MIN_FONT, Math.min(MAX_FONT, Math.floor(fit * 10) / 10)) : MAX_FONT;
  }, [lines, width]);
  const textStyle = [styles.text, { fontSize, lineHeight: Math.round(fontSize * 1.36) }];

  const copyAll = async () => {
    await Clipboard.setStringAsync(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <SafeAreaView style={ui.screen} edges={['top', 'left', 'right']}>
      <View style={ui.header}>
        <Text style={[ui.title, { flex: 1 }]} numberOfLines={1}>
          {title}
        </Text>
        <Pressable accessibilityRole="button" onPress={() => void copyAll()} style={ui.button}>
          <Text style={ui.buttonText}>{copied ? 'Copied' : 'Copy all'}</Text>
        </Pressable>
        <Pressable accessibilityRole="button" onPress={onClose} style={ui.button}>
          <Text style={ui.buttonText}>Close</Text>
        </Pressable>
      </View>
      <Text style={styles.hint}>Press and hold to select text.</Text>
      <ScrollView
        ref={scroll}
        onLayout={(e) => setWidth(e.nativeEvent.layout.width)}
        contentContainerStyle={{ paddingBottom: insets.bottom + 8 }}
        onContentSizeChange={() => scroll.current?.scrollToEnd({ animated: false })}
      >
        {Platform.OS === 'ios' ? (
          // iOS Text only copies the whole block; a read-only text view gives selection handles.
          <TextInput value={text} editable={false} multiline scrollEnabled={false} style={textStyle} />
        ) : (
          <Text selectable style={textStyle}>
            {text}
          </Text>
        )}
      </ScrollView>
    </SafeAreaView>
  );
}

const PAD = 8;

const styles = StyleSheet.create({
  hint: { color: colors.dim, fontSize: 12, paddingHorizontal: 12, paddingBottom: 4 },
  text: { color: colors.text, fontFamily: mono, paddingHorizontal: PAD },
});
