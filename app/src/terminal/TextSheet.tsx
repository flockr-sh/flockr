// Full-screen plain-text view of terminal lines with native selection and copy.
import * as Clipboard from 'expo-clipboard';
import React, { useRef, useState } from 'react';
import { Modal, Platform, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { SafeAreaView, useSafeAreaInsets } from 'react-native-safe-area-context';
import { colors, mono, ui } from '../state/ui';

interface Props {
  title: string;
  lines: string[] | null;
  onClose: () => void;
}

export function TextSheet({ title, lines, onClose }: Props) {
  const insets = useSafeAreaInsets();
  const scroll = useRef<React.ComponentRef<typeof ScrollView>>(null);
  const [copied, setCopied] = useState(false);
  const text = (lines ?? []).join('\n');

  const copyAll = async () => {
    await Clipboard.setStringAsync(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <Modal visible={lines !== null} animationType="slide" onRequestClose={onClose}>
      <SafeAreaView style={ui.screen} edges={['top', 'left', 'right']}>
        <View style={ui.header}>
          <Text style={[ui.title, { flex: 1 }]} numberOfLines={1}>
            {title}
          </Text>
          <Pressable onPress={() => void copyAll()} style={ui.button}>
            <Text style={ui.buttonText}>{copied ? 'Copied' : 'Copy all'}</Text>
          </Pressable>
          <Pressable onPress={onClose} style={ui.button}>
            <Text style={ui.buttonText}>Close</Text>
          </Pressable>
        </View>
        <Text style={styles.hint}>Press and hold to select text.</Text>
        <ScrollView
          ref={scroll}
          horizontal={false}
          contentContainerStyle={{ paddingBottom: insets.bottom + 8 }}
          onContentSizeChange={() => scroll.current?.scrollToEnd({ animated: false })}
        >
          {Platform.OS === 'ios' ? (
            // iOS Text only copies the whole block; a read-only text view gives selection handles.
            <TextInput value={text} editable={false} multiline scrollEnabled={false} style={styles.text} />
          ) : (
            <Text selectable style={styles.text}>
              {text}
            </Text>
          )}
        </ScrollView>
      </SafeAreaView>
    </Modal>
  );
}

const styles = StyleSheet.create({
  hint: { color: colors.dim, fontSize: 12, paddingHorizontal: 12, paddingBottom: 4 },
  text: { color: colors.text, fontFamily: mono, fontSize: 11, lineHeight: 15, paddingHorizontal: 8 },
});
