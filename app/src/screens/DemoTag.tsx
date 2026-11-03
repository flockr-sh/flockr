// Marks a made-up machine from demo mode so nobody takes it for a real server.
import React from 'react';
import { StyleSheet, Text } from 'react-native';
import { connections } from '../state/connections';
import { colors } from '../state/ui';

export function DemoTag({ serverId, show }: { serverId?: string; show?: boolean }) {
  const demo = show ?? (serverId ? !!connections.server(serverId)?.demo : false);
  if (!demo) return null;
  return (
    <Text style={styles.tag} accessibilityLabel="Demo machine">
      Demo
    </Text>
  );
}

const styles = StyleSheet.create({
  tag: {
    color: colors.accent,
    borderColor: colors.accent,
    borderWidth: 1,
    borderRadius: 5,
    paddingHorizontal: 5,
    paddingVertical: 1,
    fontSize: 11,
    fontWeight: '700',
    overflow: 'hidden',
    flexShrink: 0,
  },
});
