import { StyleSheet } from 'react-native';
import type { AgentStatus } from '../protocol';

export const colors = {
  bg: '#101116',
  panel: '#181a22',
  panelHi: '#20232e',
  border: '#2a2e3b',
  text: '#e6e8ef',
  dim: '#8a90a2',
  accent: '#7aa2f7',
  danger: '#f7768e',
  ok: '#9ece6a',
  warn: '#e0af68',
};

export const statusColor: Record<AgentStatus, string> = {
  blocked: '#f7768e',
  done: '#9ece6a',
  working: '#e0af68',
  idle: '#8a90a2',
  unknown: '#5c6370',
};

// Loaded in App.tsx; covers the symbols agent TUIs print, unlike the system mono fonts.
export const mono = 'JuliaMono';

export const ui = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 12,
    paddingVertical: 10,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: colors.border,
    gap: 8,
  },
  title: { color: colors.text, fontSize: 17, fontWeight: '600', flexShrink: 1 },
  sub: { color: colors.dim, fontSize: 12 },
  text: { color: colors.text, fontSize: 15 },
  button: {
    paddingHorizontal: 12,
    paddingVertical: 8,
    borderRadius: 8,
    backgroundColor: colors.panelHi,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: colors.border,
  },
  buttonText: { color: colors.text, fontSize: 14, fontWeight: '500' },
  primary: { backgroundColor: colors.accent, borderColor: colors.accent },
  primaryText: { color: '#0b0d12' },
  input: {
    color: colors.text,
    backgroundColor: colors.panel,
    borderRadius: 8,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: colors.border,
    paddingHorizontal: 10,
    paddingVertical: 8,
    fontSize: 15,
  },
});
