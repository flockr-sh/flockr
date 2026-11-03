// Draws a Subscription's grid with Skia and handles touch scroll, pan and pinch.
import { SkiaPictureView, SkPicture, useFont } from '@shopify/react-native-skia';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { LayoutChangeEvent, PanResponder, Platform, Pressable, StyleSheet, Text, View } from 'react-native';
import { Subscription, Window } from '../protocol';
import { colors } from '../state/ui';
import { Metrics, TerminalRenderer } from './renderer';

/* eslint-disable @typescript-eslint/no-require-imports */
const FONT_FILES = {
  regular: require('@expo-google-fonts/jetbrains-mono/400Regular/JetBrainsMono_400Regular.ttf'),
  bold: require('@expo-google-fonts/jetbrains-mono/700Bold/JetBrainsMono_700Bold.ttf'),
  italic: require('@expo-google-fonts/jetbrains-mono/400Regular_Italic/JetBrainsMono_400Regular_Italic.ttf'),
  boldItalic: require('@expo-google-fonts/jetbrains-mono/700Bold_Italic/JetBrainsMono_700Bold_Italic.ttf'),
  // JetBrains Mono lacks glyphs agent TUIs use (⏺ ✻ ⎿, braille spinners).
  symbols: require('../../assets/fonts/JuliaMono-Regular.ttf'),
  nerd: require('../../assets/fonts/SymbolsNerdFontMono-Regular.ttf'),
};
/* eslint-enable @typescript-eslint/no-require-imports */

/** Rows are recorded at this font size; zoom scales the composite. */
export const BASE_FONT_SIZE = 14;

export interface ViewStats {
  /** Composites pushed to the native view per second. */
  draws: Window;
  /** ms to build the composite picture. */
  compose: Window;
}

export function makeViewStats(): ViewStats {
  return { draws: new Window(1000), compose: new Window(2000) };
}

export interface TerminalLayout {
  width: number;
  height: number;
  metrics: Metrics;
}

interface Props {
  sub: Subscription | null;
  /** observe mode: scale the grid to the view width. */
  fitWidth: boolean;
  /** Zoom when not fitting width (and in fit mode: fontSize / BASE_FONT_SIZE). */
  zoom: number;
  onZoom?: (zoom: number) => void;
  onLayout?: (layout: TerminalLayout) => void;
  stats: ViewStats;
  /** Show Skia's native render-time readout. */
  debug?: boolean;
  /** A tap that did not move, at grid cell (col, row). */
  onTapCell?: (col: number, row: number) => void;
  /** Press and hold without moving. */
  onLongPress?: () => void;
}

interface SkiaViewApiT {
  setJsiProperty: (id: number, name: string, value: unknown) => void;
}

export function TerminalView({ sub, fitWidth, zoom, onZoom, onLayout, stats, debug, onTapCell, onLongPress }: Props) {
  const regular = useFont(FONT_FILES.regular, BASE_FONT_SIZE);
  const bold = useFont(FONT_FILES.bold, BASE_FONT_SIZE);
  const italic = useFont(FONT_FILES.italic, BASE_FONT_SIZE);
  const boldItalic = useFont(FONT_FILES.boldItalic, BASE_FONT_SIZE);
  const symbols = useFont(FONT_FILES.symbols, BASE_FONT_SIZE);
  const nerd = useFont(FONT_FILES.nerd, BASE_FONT_SIZE);
  const renderer = useMemo(
    () =>
      regular && bold && italic && boldItalic && symbols && nerd
        ? new TerminalRenderer({ regular, bold, italic, boldItalic, symbols, nerd })
        : null,
    [regular, bold, italic, boldItalic, symbols, nerd],
  );
  const viewRef = useRef<SkiaPictureView>(null);
  const [fallbackPicture, setFallbackPicture] = useState<SkPicture | undefined>(undefined);
  const [size, setSize] = useState({ width: 0, height: 0 });
  // Lines scrolled back into history (0 = live bottom), for the return marker.
  const [scrolledBack, setScrolledBack] = useState(0);
  const view = useRef({ width: 0, height: 0, zoom: 1, offsetX: 0, offsetY: 0, stickBottom: true });
  const frameReq = useRef<number | null>(null);
  const subRef = useRef<Subscription | null>(sub);
  subRef.current = sub;

  const effectiveZoom = useCallback(() => {
    const s = subRef.current;
    if (fitWidth && s && s.grid.cols > 0 && renderer && view.current.width > 0) {
      return view.current.width / (s.grid.cols * renderer.metrics.cellW);
    }
    return zoom;
  }, [fitWidth, zoom, renderer]);

  const clampOffsets = useCallback(() => {
    const s = subRef.current;
    const v = view.current;
    if (!s || !renderer) return;
    const contentW = s.grid.cols * renderer.metrics.cellW * v.zoom;
    const contentH = s.grid.rows * renderer.metrics.cellH * v.zoom;
    const maxX = Math.max(0, contentW - v.width);
    const maxY = Math.max(0, contentH - v.height);
    v.offsetX = Math.min(Math.max(0, v.offsetX), maxX);
    v.offsetY = v.stickBottom ? maxY : Math.min(Math.max(0, v.offsetY), maxY);
  }, [renderer]);

  const draw = useCallback(() => {
    frameReq.current = null;
    const s = subRef.current;
    if (!s || !renderer || s.grid.seq === 0 || view.current.width === 0) return;
    const t0 = performance.now();
    view.current.zoom = effectiveZoom();
    clampOffsets();
    const v = view.current;
    const pic = renderer.compose(s.grid, {
      width: v.width,
      height: v.height,
      zoom: v.zoom,
      offsetX: v.offsetX,
      offsetY: v.offsetY,
      showCursor: true,
    });
    const api = (globalThis as unknown as { SkiaViewApi?: SkiaViewApiT }).SkiaViewApi;
    const native = viewRef.current;
    const web = native as unknown as { setPicture?: (picture: SkPicture) => void } | null;
    if (Platform.OS === 'web' && web?.setPicture) {
      // The web view has no native id; it takes the picture directly.
      web.setPicture(pic);
    } else if (api && native) {
      // Skip a React render: hand the picture straight to the native view.
      api.setJsiProperty(native.nativeId, 'picture', pic);
      native.redraw();
    } else {
      setFallbackPicture(pic);
    }
    const now = performance.now();
    stats.compose.add(now - t0, now);
    stats.draws.add(1, now);
    // Ack after the picture is handed to the view, so the server's rate
    // follows what this phone actually draws.
    s.ack();
  }, [renderer, effectiveZoom, clampOffsets, stats]);

  const schedule = useCallback(() => {
    if (frameReq.current === null) frameReq.current = requestAnimationFrame(draw);
  }, [draw]);

  useEffect(
    () => () => {
      if (frameReq.current !== null) cancelAnimationFrame(frameReq.current);
    },
    [],
  );

  // Hook the subscription.
  useEffect(() => {
    if (!sub || !renderer) return;
    sub.opts.autoAck = false;
    const prev = sub.handlers;
    renderer.setTheme(sub.theme);
    setScrolledBack(sub.grid.scroll?.offset ?? 0);
    if (sub.grid.seq > 0) {
      renderer.apply(
        { full: true, shift: 0, rows: [], cursorChanged: true, scrollChanged: true, sizeChanged: true },
        sub.grid,
      );
      schedule();
    }
    sub.handlers = {
      ...prev,
      onStarted: (st) => {
        renderer.setTheme(sub.theme);
        prev.onStarted?.(st);
      },
      onUpdate: (change, grid) => {
        if (change.scrollChanged || change.full) setScrolledBack(grid.scroll?.offset ?? 0);
        const ms = renderer.apply(change, grid);
        sub.reportRecord(ms);
        prev.onUpdate?.(change, grid);
        schedule();
      },
    };
    return () => {
      sub.handlers = prev;
    };
  }, [sub, renderer, schedule]);

  // Redraw on zoom/fit changes.
  useEffect(() => {
    schedule();
  }, [fitWidth, zoom, schedule]);

  const handleLayout = useCallback(
    (e: LayoutChangeEvent) => {
      const { width, height } = e.nativeEvent.layout;
      view.current.width = width;
      view.current.height = height;
      setSize({ width, height });
      if (renderer) onLayout?.({ width, height, metrics: renderer.metrics });
      schedule();
    },
    [renderer, onLayout, schedule],
  );

  useEffect(() => {
    if (renderer && size.width > 0) onLayout?.({ width: size.width, height: size.height, metrics: renderer.metrics });
  }, [renderer, size, onLayout]);

  // On the web, onLayout doesn't fire for this view; watch its element instead.
  const container = useRef<unknown>(null);
  useEffect(() => {
    if (Platform.OS !== 'web' || !renderer || typeof ResizeObserver === 'undefined') return;
    const element = container.current as unknown as Element | null;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect;
      handleLayout({ nativeEvent: { layout: { x: 0, y: 0, width, height } } } as LayoutChangeEvent);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [renderer, handleLayout]);

  // Gestures: one finger vertical = wheel scroll (the app's own scrolling or
  // the server viewport's scrollback), horizontal =
  // pan; two fingers = pinch zoom.
  const gesture = useRef({
    axis: '' as '' | 'x' | 'y',
    accY: 0,
    lastDy: 0,
    startX: 0,
    startZoom: 1,
    startDist: 0,
    // Tap and long-press: the touch start, and whether it has moved.
    downX: 0,
    downY: 0,
    downAt: 0,
    moved: false,
    held: false,
    holdTimer: null as ReturnType<typeof setTimeout> | null,
    // Scroll acceleration and momentum: finger cell for wheel steps, and the
    // running fling after release.
    lastX: 0,
    lastY: 0,
    fling: null as ReturnType<typeof setInterval> | null,
  });
  useEffect(() => {
    const st = gesture.current;
    return () => {
      if (st.fling) clearInterval(st.fling);
    };
  }, []);
  const tapRef = useRef({ onTapCell, onLongPress });
  tapRef.current = { onTapCell, onLongPress };
  const responder = useMemo(() => {
    // Scroll `px` of drag (positive = down = back into history), as wheel
    // steps at the finger's cell so full-screen apps that draw their own
    // scrolling move too.
    const scrollBy = (px: number) => {
      const st = gesture.current;
      const s = subRef.current;
      if (!s || !renderer) return;
      st.accY += px;
      const v = view.current;
      const lineH = renderer.metrics.cellH * v.zoom;
      const lines = Math.trunc(st.accY / lineH);
      if (lines === 0) return;
      st.accY -= lines * lineH;
      const col = Math.floor((st.lastX + v.offsetX) / (renderer.metrics.cellW * v.zoom));
      const row = Math.floor((st.lastY + v.offsetY) / lineH);
      s.wheel(
        lines,
        Math.min(Math.max(0, col), Math.max(0, s.grid.cols - 1)),
        Math.min(Math.max(0, row), Math.max(0, s.grid.rows - 1)),
      );
    };
    const stopFling = () => {
      const st = gesture.current;
      if (st.fling) clearInterval(st.fling);
      st.fling = null;
    };
    return PanResponder.create({
      // Claim every touch so taps and holds reach us, not only drags.
      onStartShouldSetPanResponder: () => true,
      onMoveShouldSetPanResponder: () => true,
      onPanResponderGrant: (e) => {
        stopFling();
        const st = gesture.current;
        st.downX = e.nativeEvent.locationX;
        st.downY = e.nativeEvent.locationY;
        st.downAt = Date.now();
        st.moved = false;
        st.held = false;
        if (st.holdTimer) clearTimeout(st.holdTimer);
        st.holdTimer = setTimeout(() => {
          if (!st.moved) {
            st.held = true;
            tapRef.current.onLongPress?.();
          }
        }, 500);
        st.axis = '';
        st.accY = 0;
        st.lastDy = 0;
        st.startX = view.current.offsetX;
        st.startZoom = view.current.zoom;
        st.startDist = 0;
      },
      onPanResponderMove: (e, g) => {
        const st = gesture.current;
        if (!st.moved && (Math.abs(g.dx) > 6 || Math.abs(g.dy) > 6 || g.numberActiveTouches === 2)) {
          st.moved = true;
          if (st.holdTimer) clearTimeout(st.holdTimer);
        }
        if (!st.moved) return;
        const touches = e.nativeEvent.touches;
        if (touches.length === 2) {
          const dx = touches[0].pageX - touches[1].pageX;
          const dy = touches[0].pageY - touches[1].pageY;
          const dist = Math.hypot(dx, dy);
          if (st.startDist === 0) st.startDist = dist;
          else onZoom?.(Math.min(4, Math.max(0.2, (st.startZoom * dist) / st.startDist)));
          return;
        }
        if (!st.axis) st.axis = Math.abs(g.dx) > Math.abs(g.dy) ? 'x' : 'y';
        const s = subRef.current;
        if (!s || !renderer) return;
        if (st.axis === 'x') {
          view.current.offsetX = st.startX - g.dx;
          schedule();
        } else {
          const delta = g.dy - st.lastDy;
          st.lastDy = g.dy;
          st.lastX = e.nativeEvent.locationX;
          st.lastY = e.nativeEvent.locationY;
          scrollBy(delta * scrollAccel(g.vy));
        }
      },
      onPanResponderRelease: (_e, g) => {
        const st = gesture.current;
        if (st.holdTimer) clearTimeout(st.holdTimer);
        if (st.moved && st.axis === 'y' && Math.abs(g.vy) > FLING_MIN_VELOCITY) {
          // Keep scrolling after a flick and slow to a stop.
          let velocity = g.vy * scrollAccel(g.vy);
          st.fling = setInterval(() => {
            scrollBy(velocity * FLING_TICK_MS);
            velocity *= FLING_DECAY;
            if (Math.abs(velocity) < FLING_STOP_VELOCITY) stopFling();
          }, FLING_TICK_MS);
          return;
        }
        if (st.moved || st.held || Date.now() - st.downAt > 400 || !renderer) return;
        const v = view.current;
        const col = Math.floor((st.downX + v.offsetX) / (renderer.metrics.cellW * v.zoom));
        const row = Math.floor((st.downY + v.offsetY) / (renderer.metrics.cellH * v.zoom));
        tapRef.current.onTapCell?.(col, row);
      },
      onPanResponderTerminate: () => {
        const st = gesture.current;
        if (st.holdTimer) clearTimeout(st.holdTimer);
      },
    });
  }, [renderer, onZoom, schedule]);

  if (!renderer) {
    return (
      <View style={[styles.fill, styles.center]}>
        <Text style={{ color: colors.dim }}>Loading font…</Text>
      </View>
    );
  }

  return (
    <View ref={container as never} style={styles.fill} onLayout={handleLayout}>
      <View style={styles.fill} {...responder.panHandlers}>
        <SkiaPictureView ref={viewRef} style={styles.fill} picture={fallbackPicture} debug={debug} opaque />
      </View>
      {scrolledBack > 0 ? (
        <Pressable accessibilityRole="button" style={styles.backPill} onPress={() => sub?.scrollToBottom()}>
          <Text style={styles.backPillText}>
            {scrolledBack} {scrolledBack === 1 ? 'line' : 'lines'} back · Bottom ↓
          </Text>
        </Pressable>
      ) : null}
    </View>
  );
}

// Scroll feel: a drag moves content 1:1 when slow and up to MAX_SCROLL_ACCEL
// times faster when fast (velocity in px/ms). A release faster than
// FLING_MIN_VELOCITY keeps scrolling, slowing by FLING_DECAY per tick.
const SCROLL_ACCEL = 1.5;
const MAX_SCROLL_ACCEL = 6;
const FLING_MIN_VELOCITY = 0.3;
const FLING_STOP_VELOCITY = 0.03;
const FLING_DECAY = 0.95;
const FLING_TICK_MS = 16;

function scrollAccel(vy: number): number {
  return Math.min(MAX_SCROLL_ACCEL, 1 + SCROLL_ACCEL * Math.abs(vy));
}

const styles = StyleSheet.create({
  fill: { flex: 1, backgroundColor: colors.bg, overflow: 'hidden' },
  center: { alignItems: 'center', justifyContent: 'center' },
  backPill: {
    position: 'absolute',
    bottom: 10,
    alignSelf: 'center',
    backgroundColor: colors.panelHi,
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
    borderRadius: 16,
    paddingHorizontal: 14,
    paddingVertical: 7,
  },
  backPillText: { color: colors.text, fontSize: 13, fontWeight: '600' },
});
