import type { ConsumptionReceipt, ConsumptionTarget } from './observeConsumption';

export function continuationTarget(input: {
  receipt: ConsumptionReceipt;
  phase: string;
  tokens: readonly string[];
  liveId: string;
  requestId: string;
  streamIncarnation: string;
  sequence: number;
  text: string;
}): ConsumptionTarget {
  const { receipt, phase, tokens, liveId, requestId, streamIncarnation, sequence, text } = input;
  if (receipt.status !== 'OBSERVED' || receipt.liveId !== liveId || receipt.requestId !== requestId
    || receipt.streamIncarnation !== streamIncarnation || receipt.eventSequence !== sequence || receipt.tokenSequence !== sequence
    || receipt.text !== text || !requestId || !streamIncarnation || !Number.isSafeInteger(sequence) || sequence < 0
    || !Number.isSafeInteger(receipt.connectionEpoch) || receipt.connectionEpoch < 0
    || !phase || tokens.length === 0 || tokens.some(token => typeof token !== 'string' || token.length === 0)) {
    throw new Error('Continuation requires the exact completed consumed request');
  }
  const finalSequence = sequence + tokens.length;
  if (!Number.isSafeInteger(finalSequence)) throw new Error('Unsafe continuation sequence');
  return { phase, requestId, streamIncarnation, connectionEpoch: receipt.connectionEpoch,
    eventSequence: finalSequence, tokenSequence: finalSequence, text: text + tokens.join('') };
}
