// Web startup: page styles, then CanvasKit, then the app.
import { registerRootComponent } from 'expo';
import { LoadSkiaWeb } from '@shopify/react-native-skia/lib/module/web';

export function start(): void {
  // No focus ring on mouse clicks or text fields; a faint one for keyboard focus only.
  const style = document.createElement('style');
  style.textContent =
    '*:focus{outline:none}*:focus-visible{outline:2px solid rgba(122,162,247,.45);outline-offset:-2px}input:focus-visible,textarea:focus-visible{outline:none}';
  document.head.appendChild(style);
  // Skia draws with CanvasKit on the web. It has to load before any module
  // that uses Skia, so the app is required after it.
  void LoadSkiaWeb({ locateFile: (file: string) => `/${file}` }).then(() =>
    registerRootComponent(require('../App').default),
  );
}
