import type { Message } from '../api';
import './UserMetaMessage.css';

type MessageImage = { data: string; media_type: string };
type MessageFile = { original_name: string; size_bytes: number; stored_path?: string };

function formatMessageTime(isoStr: string): string {
  const date = new Date(isoStr);
  return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function copyObservation(text: string): void {
  void navigator.clipboard.writeText(text);
}

function formatAttachmentBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function FileChips({ files }: { files: MessageFile[] }) {
  if (files.length === 0) return null;
  return (
    <div className="message-files">
      {files.map((file, idx) => (
        <span key={`${file.stored_path ?? file.original_name}-${idx}`} className="message-file-chip" title={file.stored_path}>
          📎 {file.original_name} <span className="message-file-size">{formatAttachmentBytes(file.size_bytes)}</span>
        </span>
      ))}
    </div>
  );
}

export function UserMetaMessage({
  message,
  text,
  images,
  files,
  timestamp,
}: {
  message: Message;
  text: string;
  images: MessageImage[];
  files: MessageFile[];
  timestamp?: string;
}) {
  return (
    <section
      id={`message-${message.message_id}`}
      className="message meta user-meta-observation"
      data-sequence-id={message.sequence_id}
      aria-label="Background task observation"
    >
      <div className="message-header">
        <span className="message-header-meta">
          <span className="user-meta-observation__provenance" aria-label="System-generated background task observation">
            Background task observation
          </span>
          {timestamp && (
            <span className="message-time" title={new Date(timestamp).toLocaleString()}>
              {formatMessageTime(timestamp)}
            </span>
          )}
        </span>
        <span className="message-header-actions">
          <button
            type="button"
            className="message-copy-button"
            title="Copy system observation"
            aria-label="Copy system observation"
            onClick={() => copyObservation(text)}
          >
            Copy
          </button>
        </span>
      </div>
      <div className="message-content">
        <p>{text}</p>
        {images.length > 0 && (
          <div className="message-images">
            {images.map((img, idx) => (
              <img
                key={idx}
                src={`data:${img.media_type};base64,${img.data}`}
                alt={`Attachment ${idx + 1}`}
                className="message-image"
              />
            ))}
          </div>
        )}
        <FileChips files={files} />
      </div>
    </section>
  );
}
