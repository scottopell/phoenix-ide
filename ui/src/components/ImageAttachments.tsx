import type { ImageData } from '../api';

interface ImageAttachmentsProps {
  images: ImageData[];
  onRemove: (index: number) => void;
}

export function ImageAttachments({ images, onRemove }: ImageAttachmentsProps) {
  if (images.length === 0) return null;

  return (
    <div className="image-attachments">
      {images.map((img, idx) => (
        <div key={idx} className="image-attachment">
          <img
            src={`data:${img.media_type};base64,${img.data}`}
            alt={`Attachment ${idx + 1}`}
            className="image-thumbnail"
          />
          <button
            className="image-remove"
            onClick={() => onRemove(idx)}
            aria-label="Remove image"
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}
