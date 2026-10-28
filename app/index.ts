import './src/polyfills';
import { registerRootComponent } from 'expo';
import { Platform } from 'react-native';

if (Platform.OS === 'web') {
  // No focus ring on mouse clicks; a faint one for keyboard focus only.
  const style = document.createElement('style');
  style.textContent =
    '*:focus{outline:none}*:focus-visible{outline:2px solid rgba(122,162,247,.45);outline-offset:-2px}';
  document.head.appendChild(style);
  // Skia draws with CanvasKit on the web. It has to load before any module
  // that uses Skia, so the app is required after it.
  const { LoadSkiaWeb } = require('@shopify/react-native-skia/lib/module/web');
  void LoadSkiaWeb({ locateFile: (file: string) => `/${file}` }).then(() =>
    registerRootComponent(require('./App').default),
  );
} else {
  registerRootComponent(require('./App').default);
}
