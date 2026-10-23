import type { SseInitData } from '../../sseSchemas';

export function alignTouchInitialWatermark(init: SseInitData): SseInitData {
  if (init.transcript_coverage !== 'complete' || init.pending_events.length !== 0 || init.pending_truncated) {
    throw new Error('Touch initial watermark requires complete history without pending replay');
  }
  const floor = init.messages.reduce((max, message) => {
    if (!Number.isSafeInteger(message.sequence_id) || message.sequence_id < 0) {
      throw new Error('Invalid touch history sequence');
    }
    return Math.max(max, message.sequence_id);
  }, 0);
  return { ...init, sequence_id: floor, last_sequence_id: floor, pending_anchor_sequence_id: floor };
}
