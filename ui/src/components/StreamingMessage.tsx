import type { StreamingBuffer } from '../conversation/atom';

interface StreamingMessageProps {
  buffer: StreamingBuffer | null;
}

/**
 * Displays in-progress streaming text below the message list (REQ-UI-019, REQ-BED-025).
 *
 * Renders while the LLM is generating. When the final `sse_message` event arrives,
 * the reducer clears this buffer and appends the finalized message atomically — one
 * React render, no flicker, no duplication.
 */
export function StreamingMessage({ buffer }: StreamingMessageProps) {
  if (!buffer) return null;
  return (
    <div className="streaming-message agent-message">
      <div className="streaming-message-content">{buffer.text}</div>
      <span className="streaming-cursor" aria-hidden="true" />
    </div>
  );
}
