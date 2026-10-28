import './src/polyfills';
import { registerRootComponent } from 'expo';
import { Platform } from 'react-native';

if (Platform.OS === 'web') {
  // Skia draws with CanvasKit on the web. It has to load before any module
  // that uses Skia, so the app is required after it.
  const { LoadSkiaWeb } = require('@shopify/react-native-skia/lib/module/web');
  void LoadSkiaWeb({ locateFile: (file: string) => `/${file}` }).then(() =>
    registerRootComponent(require('./App').default),
  );
} else {
  registerRootComponent(require('./App').default);
}
