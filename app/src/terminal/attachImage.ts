// Pick a photo and shrink it to fit one `input.image` message (1 MiB, base64).
import { Alert } from 'react-native';
import { requireOptionalNativeModule } from 'expo';
import type * as ImagePickerModule from 'expo-image-picker';
import type * as ImageManipulatorModule from 'expo-image-manipulator';

/** Long edge in px. Claude downsizes larger images to this anyway. */
const MAX_EDGE = 1568;
/** Base64 budget, under the server's 1 MiB input cap with room for the JSON around it. */
const MAX_BASE64 = 1024 * 1024 - 8 * 1024;
const QUALITIES = [0.8, 0.6, 0.4, 0.25];

/**
 * Whether this build has the picker and manipulator native code. Builds made
 * before they were added load the same JS from Metro, so both modules are
 * required lazily: importing them on such a build throws at load.
 */
export function canAttachImages(): boolean {
  return !!requireOptionalNativeModule('ExponentImagePicker') && !!requireOptionalNativeModule('ExpoImageManipulator');
}

export interface PreparedImage {
  format: 'jpg';
  data: string;
}

/** Asks library or camera, then returns the image as base64 JPEG, or null when canceled. */
export async function pickImage(): Promise<PreparedImage | null> {
  const source = await new Promise<'library' | 'camera' | null>((resolve) =>
    Alert.alert(
      'Attach image',
      undefined,
      [
        { text: 'Photo library', onPress: () => resolve('library') },
        { text: 'Camera', onPress: () => resolve('camera') },
        { text: 'Cancel', style: 'cancel', onPress: () => resolve(null) },
      ],
      // Android: a tap outside the dialog dismisses it without a button.
      { cancelable: true, onDismiss: () => resolve(null) },
    ),
  );
  if (!source) return null;
  const ImagePicker = require('expo-image-picker') as typeof ImagePickerModule;
  const options: ImagePickerModule.ImagePickerOptions = { mediaTypes: ['images'], quality: 1 };
  let result: ImagePickerModule.ImagePickerResult;
  if (source === 'camera') {
    const permission = await ImagePicker.requestCameraPermissionsAsync();
    if (!permission.granted) throw new Error('camera permission denied');
    result = await ImagePicker.launchCameraAsync(options);
  } else {
    result = await ImagePicker.launchImageLibraryAsync(options);
  }
  const asset = result.canceled ? undefined : result.assets[0];
  if (!asset) return null;
  return { format: 'jpg', data: await shrink(asset.uri, asset.width, asset.height) };
}

async function shrink(uri: string, width: number, height: number): Promise<string> {
  const { ImageManipulator, SaveFormat } = require('expo-image-manipulator') as typeof ImageManipulatorModule;
  const context = ImageManipulator.manipulate(uri);
  if (width > MAX_EDGE || height > MAX_EDGE) {
    context.resize(width >= height ? { width: MAX_EDGE } : { height: MAX_EDGE });
  }
  const image = await context.renderAsync();
  for (const compress of QUALITIES) {
    const saved = await image.saveAsync({ format: SaveFormat.JPEG, compress, base64: true });
    if (saved.base64 && saved.base64.length <= MAX_BASE64) return saved.base64;
  }
  throw new Error('image is too large to send');
}
