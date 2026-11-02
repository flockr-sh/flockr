// Clipboard images on the web: shrink a pasted image to fit one
// `input.image` message, like the phone does with picked photos.
import { MAX_BASE64, MAX_EDGE, QUALITIES } from '../terminal/attachImage';

const FORMATS: Record<string, string> = {
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'image/bmp': 'bmp',
};

/** The first image in a paste, if any. */
export function pastedImage(data: DataTransfer | null): File | null {
  for (const item of Array.from(data?.items ?? [])) {
    if (item.kind === 'file' && item.type.startsWith('image/')) return item.getAsFile();
  }
  return null;
}

/** The image as base64 with its format: as is when it fits, else a smaller JPEG. */
export async function prepareImage(file: File): Promise<{ format: string; data: string }> {
  const format = FORMATS[file.type];
  const bitmap = await createImageBitmap(file);
  const small = bitmap.width <= MAX_EDGE && bitmap.height <= MAX_EDGE;
  if (format && small) {
    const data = await base64(file);
    if (data.length <= MAX_BASE64) return { format, data };
  }
  const scale = Math.min(1, MAX_EDGE / Math.max(bitmap.width, bitmap.height));
  const canvas = document.createElement('canvas');
  canvas.width = Math.round(bitmap.width * scale);
  canvas.height = Math.round(bitmap.height * scale);
  canvas.getContext('2d')?.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
  for (const quality of QUALITIES) {
    const data = canvas.toDataURL('image/jpeg', quality).split(',')[1] ?? '';
    if (data && data.length <= MAX_BASE64) return { format: 'jpg', data };
  }
  throw new Error('image is too large to send');
}

function base64(file: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).split(',')[1] ?? '');
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}
