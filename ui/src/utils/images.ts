import type { ImageData } from '../api';

export const SUPPORTED_IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];
export const MAX_IMAGE_SIZE = 5 * 1024 * 1024; // 5MB

export async function fileToBase64(file: File): Promise<ImageData> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result as string;
      const base64 = result.split(',')[1] ?? '';
      resolve({ data: base64, media_type: file.type });
    };
    reader.onerror = () => reject(new Error('Failed to read file'));
    reader.readAsDataURL(file);
  });
}

export function filterValidImages(files: File[]): File[] {
  return files.filter(file => {
    if (!SUPPORTED_IMAGE_TYPES.includes(file.type)) {
      console.warn(`Unsupported image type: ${file.type}`);
      return false;
    }
    if (file.size > MAX_IMAGE_SIZE) {
      console.warn(`Image too large: ${file.name}`);
      return false;
    }
    return true;
  });
}

export async function processImageFiles(files: File[]): Promise<ImageData[]> {
  const validFiles = filterValidImages(files);
  return Promise.all(validFiles.map(fileToBase64));
}
