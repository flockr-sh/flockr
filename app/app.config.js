// Expo config. Mirrors the Corvi app's setup (variants, EAS profiles, ATS).
const APP_VARIANT = process.env.APP_VARIANT || 'development';
const IS_DEV = APP_VARIANT === 'development';
// EAS project @dmkenney/flockr-mobile. Push tokens need it.
const EAS_PROJECT_ID = process.env.EAS_PROJECT_ID || 'a9e72b44-a4d7-4c9d-bffd-9b7addac30b3';

module.exports = {
  expo: {
    name: IS_DEV ? 'flockr (dev)' : 'flockr',
    slug: 'flockr-mobile',
    scheme: 'flockr',
    version: '0.1.0',
    orientation: 'default',
    userInterfaceStyle: 'dark',
    runtimeVersion: { policy: 'appVersion' },
    ios: {
      bundleIdentifier: IS_DEV ? 'sh.flockr.app.dev' : 'sh.flockr.app',
      supportsTablet: true,
      infoPlist: {
        ITSAppUsesNonExemptEncryption: false,
        // mobile.v2 is plain ws:// (no TLS); the payload is Noise encrypted
        // in the app. iOS blocks cleartext sockets without this.
        NSAppTransportSecurity: {
          NSAllowsArbitraryLoads: true,
          NSAllowsLocalNetworking: true,
        },
        NSLocalNetworkUsageDescription: 'flockr connects to your flockr server on your private network.',
      },
    },
    android: {
      package: IS_DEV ? 'sh.flockr.app.dev' : 'sh.flockr.app',
      predictiveBackGestureEnabled: false,
    },
    web: {},
    plugins: [
      'expo-font',
      'expo-status-bar',
      // Tokens are read without Face ID.
      ['expo-secure-store', { faceIDPermission: false }],
      [
        'expo-camera',
        {
          cameraPermission: 'flockr uses the camera to scan the pairing QR code shown by `flockr mobile pair`.',
          microphonePermission: false,
          recordAudioAndroid: false,
        },
      ],
      [
        'expo-notifications',
        {
          // Blocked/Done alerts.
          defaultChannel: 'agents',
          // TestFlight and App Store builds use Apple's production push service.
          mode: 'production',
        },
      ],
      [
        'expo-build-properties',
        {
          // Plain ws:// to the gateway (see NSAppTransportSecurity above).
          android: { usesCleartextTraffic: true },
        },
      ],
    ],
    extra: {
      appVariant: APP_VARIANT,
      ...(EAS_PROJECT_ID ? { eas: { projectId: EAS_PROJECT_ID } } : {}),
    },
  },
};
