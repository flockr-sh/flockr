// Perf readout: the prototype's main output. All numbers come from
// performance.now() on the JS thread; see README "Reading the perf overlay".
import React, { useEffect, useRef, useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { GatewayClient, Subscription, Window } from '../protocol';
import { mono } from '../state/ui';
import type { ViewStats } from './TerminalView';

interface Props {
  client: GatewayClient | null;
  sub: Subscription | null;
  view: ViewStats;
}

const f1 = (n: number | null | undefined) => (n === null || n === undefined ? '–' : n.toFixed(1));
const f0 = (n: number | null | undefined) => (n === null || n === undefined ? '–' : n.toFixed(0));

export function PerfOverlay({ client, sub, view }: Props) {
  const [, setTick] = useState(0);
  const jsFrames = useRef(new Window(1000));
  const jsLongest = useRef(new Window(2000));

  // JS-thread frame rate: rAF callbacks per second and the longest gap.
  useEffect(() => {
    let req = 0;
    let last = performance.now();
    const loop = (t: number) => {
      jsFrames.current.add(1, t);
      jsLongest.current.add(t - last, t);
      last = t;
      req = requestAnimationFrame(loop);
    };
    req = requestAnimationFrame(loop);
    const timer = setInterval(() => setTick((x) => x + 1), 500);
    return () => {
      cancelAnimationFrame(req);
      clearInterval(timer);
    };
  }, []);

  const s = sub?.stats.snapshot();
  const g = sub?.grid;
  const lines = [
    `draw ${f0(view.draws.ratePerSec())}/s  js ${f0(jsFrames.current.ratePerSec())}fps (max gap ${f0(jsLongest.current.max())}ms)`,
    `recv ${f0(s?.updatesPerSec)}/s (full ${f0(s?.framesPerSec)})  ${f1((s?.bytesPerSec ?? 0) / 1024)} KB/s  rows ${f0(s?.rowsPerSec)}/s`,
    `parse ${f1(s?.parseMs)}  apply ${f1(s?.applyMs)}  record ${f1(s?.recordMs)}  compose ${f1(view.compose.avg())} ms`,
    `worst parse+apply+record ${f1(s?.worstMs)} ms  resyncs ${s?.resyncs ?? 0}`,
    `rtt ping ${f1(client?.rtt.last)} (avg ${f1(client?.rtt.avg())})  input.ok ${f1(client?.inputRtt.last)}  echo ${f1(sub?.echo.last)} ms`,
    `grid ${g?.cols ?? 0}x${g?.rows ?? 0}  seq ${g?.seq ?? 0}  styles ${g?.styles.length ?? 0}  link ${f1((client?.bytesIn.ratePerSec() ?? 0) / 1024)} KB/s`,
  ];
  return (
    <View pointerEvents="none" style={styles.box}>
      {lines.map((l, i) => (
        <Text key={i} style={styles.text}>
          {l}
        </Text>
      ))}
    </View>
  );
}

const styles = StyleSheet.create({
  // A strip above the terminal, not over it: an overlay hid the top rows.
  box: { backgroundColor: '#000', paddingHorizontal: 8, paddingVertical: 4 },
  text: { color: '#a6e3a1', fontFamily: mono, fontSize: 10, lineHeight: 13 },
});
