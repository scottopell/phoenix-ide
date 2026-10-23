import type { ConversationStore } from '../../conversation/ConversationStore';

export type ConsumptionTarget = {
  phase: string;
  requestId: string;
  eventSequence: number;
  tokenSequence: number;
  text: string;
  connectionEpoch: number;
  streamIncarnation: string;
};
export type ConsumptionReceipt =
  | { status: 'UNOBSERVED'; phase: string | null; reason: string }
  | { status: 'OBSERVED'; phase: string; liveId: string; key: string;
      connectionEpoch: number; streamIncarnation: string;
      eventSequence: number; tokenSequence: number; requestId: string; text: string };

export function observeConsumption(
  store: ConversationStore,
  liveId: string,
  publish: (receipt: ConsumptionReceipt) => void,
) {
  let active = true;
  let notifications = 0;
  let target: ConsumptionTarget | null = null;
  let identity: string | null = null;
  let watermark: { event: number; requestId: string | null; token: number } | null = null;
  let current: { event: number; requestId: string | null; token: number; connectionEpoch: number; streamIncarnation: string } | null = null;
  const unseen = (reason: string) => publish({ status: 'UNOBSERVED', phase: target?.phase ?? null, reason });
  const sample = () => {
    current = null;
    if (!active) return;
    const key = store.slugForId(liveId);
    const atom = key === undefined ? undefined : store.peekSnapshot(key);
    if (key === undefined || !atom || atom.conversation?.id !== liveId || atom.conversationId !== liveId) {
      identity = null;
      target = null;
      unseen('live identity unavailable');
      return;
    }
    if (atom.connectionEpoch === null || atom.streamIncarnation === null || atom.connectionState !== 'live') {
      identity = null;
      target = null;
      unseen('live connection unavailable');
      return;
    }
    if (atom.eventGap !== null || !Number.isSafeInteger(atom.lastAppliedEventSeq) || atom.lastAppliedEventSeq < 0) {
      identity = null;
      target = null;
      watermark = null;
      unseen('event continuity unavailable; phase must be armed again');
      return;
    }
    current = { event: atom.lastAppliedEventSeq, requestId: atom.streamingBuffer?.requestId ?? null,
      token: atom.streamingBuffer?.lastSequence ?? -1,
      connectionEpoch: atom.connectionEpoch, streamIncarnation: atom.streamIncarnation };
    const nextIdentity = JSON.stringify([key, atom.connectionEpoch, atom.streamIncarnation]);
    if (identity !== nextIdentity) {
      identity = nextIdentity;
      target = null;
      unseen('identity changed; phase must be armed again');
      return;
    }
    if (!target || !watermark) { unseen('no armed phase'); return; }
    if (atom.connectionEpoch !== target.connectionEpoch || atom.streamIncarnation !== target.streamIncarnation) {
      target = null;
      watermark = null;
      unseen('target connection changed; phase must be armed again');
      return;
    }
    const buffer = atom.streamingBuffer;
    if (atom.lastAppliedEventSeq <= watermark.event || !buffer || buffer.requestId !== target.requestId
      || (buffer.requestId === watermark.requestId && buffer.lastSequence <= watermark.token)
      || atom.lastAppliedEventSeq < target.eventSequence || buffer.lastSequence !== target.tokenSequence
      || buffer.text !== target.text) {
      unseen('reduced state does not match target');
      return;
    }
    publish({ status: 'OBSERVED', phase: target.phase, liveId, key,
      connectionEpoch: atom.connectionEpoch, streamIncarnation: atom.streamIncarnation,
      eventSequence: atom.lastAppliedEventSeq, tokenSequence: buffer.lastSequence,
      requestId: buffer.requestId, text: buffer.text });
  };
  const unsubscribe = store.subscribeAny(() => { notifications++; sample(); });
  sample();
  return {
    diagnostic() {
      const key = store.slugForId(liveId);
      const atom = key === undefined ? undefined : store.peekSnapshot(key);
      const buffer = atom?.streamingBuffer;
      return {
        active, notifications, indexed: key !== undefined, atomPresent: Boolean(atom),
        identityMatches: Boolean(atom && atom.conversation?.id === liveId && atom.conversationId === liveId),
        connectionState: atom?.connectionState ?? null,
        connectionEpoch: atom?.connectionEpoch ?? null,
        incarnationPresent: Boolean(atom?.streamIncarnation),
        eventGap: atom ? atom.eventGap !== null : null,
        eventSequence: atom?.lastAppliedEventSeq ?? null,
        tokenSequence: buffer?.lastSequence ?? null,
        phaseType: atom?.phase.type ?? null,
        armed: target !== null, watermarkPresent: watermark !== null,
        targetEpochMatches: target && atom ? target.connectionEpoch === atom.connectionEpoch : null,
        targetIncarnationMatches: target && atom ? target.streamIncarnation === atom.streamIncarnation : null,
        requestMatches: target && buffer ? target.requestId === buffer.requestId : null,
        textMatches: target && buffer ? target.text === buffer.text : null,
        targetEventSequence: target?.eventSequence ?? null,
        targetTokenSequence: target?.tokenSequence ?? null,
      };
    },
    arm(next: Omit<ConsumptionTarget, 'connectionEpoch' | 'streamIncarnation'> & Partial<Pick<ConsumptionTarget, 'connectionEpoch' | 'streamIncarnation'>>) {
      if (!active) return;
      target = null;
      watermark = null;
      unseen('previous phase invalidated');
      sample();
      if (identity === null || current === null) return;
      if (!next.phase || !next.requestId
        || !Number.isSafeInteger(next.eventSequence) || next.eventSequence < 0
        || !Number.isSafeInteger(next.tokenSequence) || next.tokenSequence < 0) {
        target = null;
        unseen('malformed target');
        return;
      }
      const before = current as { event: number; requestId: string | null; token: number; connectionEpoch: number; streamIncarnation: string };
      if (!Number.isSafeInteger(before.connectionEpoch) || before.connectionEpoch < 0
        || !before.streamIncarnation) {
        unseen('invalid actual connection identity');
        return;
      }
      if ((next.connectionEpoch !== undefined && next.connectionEpoch !== before.connectionEpoch)
        || (next.streamIncarnation !== undefined && next.streamIncarnation !== before.streamIncarnation)) {
        unseen('expected connection does not match live state');
        return;
      }
      if (next.eventSequence <= before.event
        || (next.requestId === before.requestId && next.tokenSequence <= before.token)) {
        unseen('target must require fresh post-arm consumption');
        return;
      }
      watermark = { ...before };
      target = { ...next, connectionEpoch: before.connectionEpoch, streamIncarnation: before.streamIncarnation };
      unseen('armed; awaiting fresh reduced state');
    },
    disarm() {
      if (!active) return;
      target = null;
      watermark = null;
      unseen('phase ended');
    },
    dispose() {
      if (!active) return;
      active = false;
      unsubscribe();
      target = null;
      identity = null;
      watermark = null;
      current = null;
      publish({ status: 'UNOBSERVED', phase: null, reason: 'observer disposed' });
    },
  };
}
