import { BarcodeScanningResult, CameraView, useCameraPermissions } from 'expo-camera';
import React, { useRef, useState } from 'react';
import { Keyboard, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { connections } from '../state/connections';
import type { Nav } from '../state/nav';
import { colors, ui } from '../state/ui';
import { GatewayError, parsePairingUrl } from '../protocol';

const ERRORS: Record<string, string> = {
  pairing_failed: 'The pairing code is wrong, expired, or already used. Run `flockr mobile pair` again.',
  too_many_devices: 'This server has reached its device limit. Revoke one with `flockr mobile devices revoke`.',
  server_mismatch: 'The server at that address is not the one that made this code.',
  timeout: 'No answer from the server. Is the gateway enabled and is the phone on the same NetBird/LAN network?',
  unsupported_version: 'This app and the server do not share a protocol version.',
};

export function PairScreen({ nav, initialUrl }: { nav: Nav; initialUrl?: string }) {
  const [permission, requestPermission] = useCameraPermissions();
  const [text, setText] = useState(initialUrl ?? '');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [scanning, setScanning] = useState(!initialUrl);
  const handled = useRef(false);

  const pair = async (url: string) => {
    Keyboard.dismiss();
    setError(null);
    let payload;
    try {
      payload = parsePairingUrl(url, Math.floor(Date.now() / 1000));
    } catch (e) {
      const msg = (e as Error).message;
      setError(
        msg === 'pairing code expired'
          ? 'This pairing code has expired. Run `flockr mobile pair` again.'
          : 'That is not a complete flockr pairing link. Copy the whole flockr://pair?p=… line again.',
      );
      handled.current = false;
      return;
    }
    setBusy(true);
    setScanning(false);
    try {
      const server = await connections.pair(payload);
      nav.reset([{ name: 'servers' }, { name: 'agents', serverId: server.server_id }]);
    } catch (e) {
      const code = e instanceof GatewayError ? e.code : '';
      setError(ERRORS[code] ?? `Pairing failed: ${(e as Error).message}`);
      handled.current = false;
    } finally {
      setBusy(false);
    }
  };

  const onScan = (r: BarcodeScanningResult) => {
    if (handled.current || busy) return;
    if (!r.data.startsWith('flockr://pair')) return;
    handled.current = true;
    void pair(r.data);
  };

  return (
    <SafeAreaView style={ui.screen}>
      <View style={ui.header}>
        <Pressable accessibilityRole="button" onPress={() => nav.pop()} style={ui.button}>
          <Text style={ui.buttonText}>‹</Text>
        </Pressable>
        <Text style={[ui.title, { flex: 1 }]}>Pair a server</Text>
      </View>
      <ScrollView contentContainerStyle={{ padding: 12, gap: 12 }} keyboardShouldPersistTaps="handled">
        <Text style={ui.sub}>
          On your computer run `flockr mobile pair` and scan the QR code, or paste the flockr://pair link.
        </Text>
        {scanning ? (
          permission?.granted ? (
            <View style={styles.cameraBox}>
              <CameraView
                style={StyleSheet.absoluteFill}
                facing="back"
                barcodeScannerSettings={{ barcodeTypes: ['qr'] }}
                onBarcodeScanned={onScan}
              />
            </View>
          ) : (
            <Pressable
              accessibilityRole="button"
              style={[ui.button, ui.primary]}
              onPress={() => void requestPermission()}
            >
              <Text style={[ui.buttonText, ui.primaryText]}>
                {permission?.canAskAgain === false ? 'Camera blocked in Settings' : 'Allow camera to scan'}
              </Text>
            </Pressable>
          )
        ) : (
          <Pressable accessibilityRole="button" style={ui.button} onPress={() => setScanning(true)}>
            <Text style={ui.buttonText}>Scan QR code</Text>
          </Pressable>
        )}
        <TextInput
          style={[ui.input, { minHeight: 70 }]}
          value={text}
          onChangeText={setText}
          placeholder="flockr://pair?p=…"
          placeholderTextColor={colors.dim}
          autoCapitalize="none"
          autoCorrect={false}
          spellCheck={false}
          autoComplete="off"
          importantForAutofill="no"
          multiline
        />
        <Pressable
          accessibilityRole="button"
          disabled={busy || !text.trim()}
          style={[ui.button, ui.primary, (busy || !text.trim()) && { opacity: 0.5 }]}
          onPress={() => void pair(text)}
        >
          <Text style={[ui.buttonText, ui.primaryText]}>{busy ? 'Pairing…' : 'Pair with pasted link'}</Text>
        </Pressable>
        {error ? <Text style={{ color: colors.danger }}>{error}</Text> : null}
        <Text style={styles.note}>
          The connection is plain WebSocket (no TLS). Only pair over an encrypted private network such as NetBird or
          WireGuard. Anyone who can read traffic on that network can read the terminal and the device token.
        </Text>
      </ScrollView>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  cameraBox: { height: 320, borderRadius: 12, overflow: 'hidden', backgroundColor: '#000' },
  note: { color: colors.dim, fontSize: 12, lineHeight: 17 },
});
