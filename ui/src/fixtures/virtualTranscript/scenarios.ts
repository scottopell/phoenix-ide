import { buildTranscriptLayout } from '../../conversation/virtualTranscriptLayout';
import type {
  VirtualTranscriptAliasLookup,
  VirtualTranscriptScenario,
  VirtualTranscriptScenarioId,
  VirtualTranscriptSnapshot,
  VirtualTranscriptUnit,
  VirtualTranscriptVisibleRange,
  VirtualTranscriptViewport,
} from './types';

function unit(
  key: string,
  canonicalMessageId: string,
  estimatedExtent: number,
  measuredExtent: number,
  text: string,
  aliasMessageIds: readonly string[] = [],
): VirtualTranscriptUnit {
  return {
    key,
    role: 'agent',
    canonicalMessageId,
    aliasMessageIds,
    estimatedExtent,
    measuredExtent,
    text,
  };
}

function viewport(offset: number, extent: number): VirtualTranscriptViewport {
  return { offset, extent };
}

function visibleRange(units: readonly VirtualTranscriptUnit[], currentViewport: VirtualTranscriptViewport): VirtualTranscriptVisibleRange {
  const layout = buildTranscriptLayout({
    keys: units.map((item) => item.key),
    estimatedExtent: (_key, index) => units[index]?.estimatedExtent ?? 0,
    measuredExtents: new Map(units.flatMap((item) => item.measuredExtent === undefined ? [] : [[item.key, item.measuredExtent] as const])),
  });
  return layout.rangeForViewport({
    viewportOffset: currentViewport.offset,
    viewportExtent: currentViewport.extent,
    overscanExtent: 0,
  }) ?? { startIndex: 0, endIndex: 0 };
}

function snapshot(
  revision: string,
  transcriptGeneration: number,
  units: readonly VirtualTranscriptUnit[],
  currentViewport: VirtualTranscriptViewport,
  options: {
    readingAnchor?: string;
    followTail?: boolean;
  } = {},
): VirtualTranscriptSnapshot {
  return {
    revision,
    transcriptGeneration,
    units,
    viewport: currentViewport,
    ...(options.readingAnchor
      ? { readingAnchor: { kind: 'message' as const, messageId: options.readingAnchor } }
      : {}),
    followTail: options.followTail ?? false,
    visibleRange: visibleRange(units, currentViewport),
  };
}

function offsetForMessage(snapshotData: VirtualTranscriptSnapshot, messageId: string): number {
  const layout = buildTranscriptLayout({
    keys: snapshotData.units.map((item) => item.key),
    estimatedExtent: (_key, index) => snapshotData.units[index]?.estimatedExtent ?? 0,
    measuredExtents: new Map(snapshotData.units.flatMap((item) => item.measuredExtent === undefined ? [] : [[item.key, item.measuredExtent] as const])),
  });
  const unit = snapshotData.units.find((item) => item.canonicalMessageId === messageId || item.aliasMessageIds?.includes(messageId));
  if (!unit) throw new Error(`Unknown message id ${messageId}`);
  return layout.offsetForKey(unit.key) ?? 0;
}

function totalExtent(snapshotData: VirtualTranscriptSnapshot): number {
  return buildTranscriptLayout({
    keys: snapshotData.units.map((item) => item.key),
    estimatedExtent: (_key, index) => snapshotData.units[index]?.estimatedExtent ?? 0,
    measuredExtents: new Map(snapshotData.units.flatMap((item) => item.measuredExtent === undefined ? [] : [[item.key, item.measuredExtent] as const])),
  }).totalExtent;
}

const prefixBeforeUnits = [
  unit('m-040', 'msg-040', 64, 64, 'Earlier summary'),
  unit('m-041', 'msg-041', 72, 72, 'Earlier summary'),
  unit('m-042', 'msg-042', 320, 320, 'Tall reasoning block that spans several viewport heights.'),
  unit('m-043', 'msg-043', 88, 88, 'Anchor message that should stay pinned after history loads.'),
  unit('m-044', 'msg-044', 104, 104, 'Later reply'),
] as const;

const prefixAfterUnits = [
  unit('m-036', 'msg-036', 76, 76, 'Newly loaded prefix 1'),
  unit('m-037', 'msg-037', 68, 68, 'Newly loaded prefix 2'),
  unit('m-038', 'msg-038', 84, 84, 'Newly loaded prefix 3'),
  ...prefixBeforeUnits,
] as const;

const resizeBeforeUnits = [
  unit('m-090', 'msg-090', 90, 90, 'Prelude'),
  unit('m-091', 'msg-091', 110, 110, 'Measured item above anchor before image decode.'),
  unit('m-092', 'msg-092', 70, 70, 'Second measured item above anchor before image decode.'),
  unit('m-093', 'msg-093', 120, 120, 'Anchor row stays in place while earlier rows resize.'),
  unit('m-094', 'msg-094', 80, 80, 'Reply after anchor'),
] as const;

const resizeAfterUnits = [
  unit('m-090', 'msg-090', 90, 90, 'Prelude'),
  unit('m-091', 'msg-091', 110, 180, 'Measured item above anchor after image decode.'),
  unit('m-092', 'msg-092', 70, 130, 'Second measured item above anchor after image decode.'),
  unit('m-093', 'msg-093', 120, 120, 'Anchor row stays in place while earlier rows resize.'),
  unit('m-094', 'msg-094', 80, 80, 'Reply after anchor'),
] as const;

const navigationUnits = [
  unit('m-200', 'msg-200', 70, 70, 'Conversation intro'),
  unit('m-201', 'msg-201', 92, 92, 'Canonical target message', ['alias-msg-201', 'legacy-anchor-201']),
  unit('m-202', 'msg-202', 96, 96, 'Follow-up after canonical target'),
] as const;

const streamBeforeUnits = [
  unit('m-300', 'msg-300', 84, 84, 'Earlier stream block'),
  unit('m-301', 'msg-301', 84, 84, 'Older stable block'),
  unit('m-302', 'msg-302', 84, 84, 'Reader anchor block'),
  unit('m-303', 'msg-303', 84, 84, 'Latest finalized block'),
] as const;

const streamAfterUnits = [
  ...streamBeforeUnits,
  unit('m-304', 'msg-304', 84, 84, 'Fresh streamed block'),
  unit('m-305', 'msg-305', 84, 84, 'Newest streamed block'),
] as const;

const supersessionUnits = [
  unit('m-400', 'msg-400', 72, 72, 'Earlier context'),
  unit('m-401', 'msg-401', 72, 72, 'Older anchor request target'),
  unit('m-402', 'msg-402', 72, 72, 'Newer anchor request target'),
  unit('m-403', 'msg-403', 72, 72, 'Later context'),
] as const;

const prefixBefore = snapshot('prefix-before', 4, prefixBeforeUnits, viewport(360, 180), {
  readingAnchor: 'msg-043',
});
const prefixAfter = snapshot('prefix-after', 5, prefixAfterUnits, viewport(588, 180), {
  readingAnchor: 'msg-043',
});

const resizeBefore = snapshot('resize-before', 8, resizeBeforeUnits, viewport(225, 160), {
  readingAnchor: 'msg-093',
});
const resizeAfter = snapshot('resize-after', 8, resizeAfterUnits, viewport(355, 160), {
  readingAnchor: 'msg-093',
});

const aliasSnapshot = snapshot('alias-navigation', 3, navigationUnits, viewport(150, 120), {});
const orphanSnapshot = snapshot('orphan-target', 3, navigationUnits, viewport(150, 120), {});

const streamReadingBefore = snapshot('stream-reading-before', 11, streamBeforeUnits, viewport(120, 168), {
  readingAnchor: 'msg-302',
  followTail: false,
});
const streamReadingAfter = snapshot('stream-reading-after', 12, streamAfterUnits, viewport(120, 168), {
  readingAnchor: 'msg-302',
  followTail: false,
});

const streamFollowingBefore = snapshot('stream-following-before', 11, streamBeforeUnits, viewport(168, 168), {
  followTail: true,
});
const streamFollowingAfter = snapshot('stream-following-after', 12, streamAfterUnits, viewport(336, 168), {
  followTail: true,
});

const supersessionBefore = snapshot('supersession-before', 6, supersessionUnits, viewport(72, 144), {
  readingAnchor: 'msg-401',
});
const supersessionAfter = snapshot('supersession-after', 6, supersessionUnits, viewport(144, 144), {
  readingAnchor: 'msg-402',
});

const aliasLookups: readonly VirtualTranscriptAliasLookup[] = [
  { requestedMessageId: 'alias-msg-201', resolvedMessageKey: 'm-201' },
  { requestedMessageId: 'legacy-anchor-201', resolvedMessageKey: 'm-201' },
  { requestedMessageId: 'msg-201', resolvedMessageKey: 'm-201' },
];

const scenarios = [
  {
    id: 'prefix-insertion-within-tall-unit',
    title: 'Restore reader anchor after prefix insertion while scrolled inside a tall unit',
    story: 'The reader is partway through a tall transcript unit when earlier history loads above it, and the anchor row stays at the same visual position.',
    tags: ['continuity', 'prefix-load', 'tall-unit'],
    before: prefixBefore,
    after: prefixAfter,
    expectation: {
      kind: 'restore_anchor_after_prefix_insertion',
      anchorMessageId: 'msg-043',
      anchorKey: 'm-043',
      previousAnchorOffset: offsetForMessage(prefixBefore, 'msg-043'),
      nextAnchorOffset: offsetForMessage(prefixAfter, 'msg-043'),
      insertedKeys: ['m-036', 'm-037', 'm-038'],
      preservedViewportDelta: prefixAfter.viewport.offset - prefixBefore.viewport.offset,
    },
  },
  {
    id: 'resize-above-anchor',
    title: 'Hold the same anchor when measured content above it grows',
    story: 'Images and wrapped content finish measuring above the reader anchor, and the viewport shifts just enough to keep the anchored message in place.',
    tags: ['continuity', 'resize', 'measurement'],
    before: resizeBefore,
    after: resizeAfter,
    expectation: {
      kind: 'preserve_anchor_across_resize',
      anchorMessageId: 'msg-093',
      anchorKey: 'm-093',
      previousAnchorOffset: offsetForMessage(resizeBefore, 'msg-093'),
      nextAnchorOffset: offsetForMessage(resizeAfter, 'msg-093'),
      resizedKeys: ['m-091', 'm-092'],
      preservedViewportDelta: resizeAfter.viewport.offset - resizeBefore.viewport.offset,
    },
  },
  {
    id: 'alias-navigation',
    title: 'Navigate to a target through alias resolution',
    story: 'A saved deep link points at a historical alias id, and the transcript resolves it to the canonical row before scrolling there.',
    tags: ['navigation', 'alias'],
    before: aliasSnapshot,
    after: aliasSnapshot,
    aliasLookups,
    expectation: {
      kind: 'resolve_alias_navigation',
      requestedMessageId: 'alias-msg-201',
      resolvedMessageKey: 'm-201',
      targetIndex: 1,
      targetOffset: offsetForMessage(aliasSnapshot, 'alias-msg-201'),
    },
  },
  {
    id: 'orphan-target',
    title: 'Surface an orphan target when no transcript row matches the request',
    story: 'A deep link asks for a message id that is no longer present, and the transcript reports a missing target instead of jumping unpredictably.',
    tags: ['navigation', 'failure'],
    before: orphanSnapshot,
    after: orphanSnapshot,
    aliasLookups: [{ requestedMessageId: 'missing-msg-999', resolvedMessageKey: null }],
    expectation: {
      kind: 'report_orphan_target',
      requestedMessageId: 'missing-msg-999',
      reason: 'target_missing',
    },
  },
  {
    id: 'streaming-growth-reading',
    title: 'Append streamed content without displacing an active reader',
    story: 'New transcript rows stream in while the user is reading older content, and the viewport stays fixed on the reader anchor instead of following the tail.',
    tags: ['streaming', 'reading'],
    before: streamReadingBefore,
    after: streamReadingAfter,
    expectation: {
      kind: 'stream_append_without_reposition',
      appendedKeys: ['m-304', 'm-305'],
      preservedViewportOffset: streamReadingBefore.viewport.offset,
    },
  },
  {
    id: 'streaming-growth-following',
    title: 'Follow tail growth when the reader is already pinned to the end',
    story: 'New transcript rows stream in while the user is following the live tail, and the viewport advances so the newest content remains visible.',
    tags: ['streaming', 'follow-tail'],
    before: streamFollowingBefore,
    after: streamFollowingAfter,
    expectation: {
      kind: 'stream_append_and_follow_tail',
      appendedKeys: ['m-304', 'm-305'],
      previousViewportOffset: streamFollowingBefore.viewport.offset,
      nextViewportOffset: streamFollowingAfter.viewport.offset,
      nextViewportEnd: streamFollowingAfter.viewport.offset + streamFollowingAfter.viewport.extent,
      totalExtent: totalExtent(streamFollowingAfter),
    },
  },
  {
    id: 'supersession',
    title: 'Drop an older restore command when a newer one supersedes it',
    story: 'A newer restore command arrives before the older one applies, and the transcript scrolls only to the latest anchor while treating the prior request as superseded.',
    tags: ['continuity', 'supersession'],
    before: supersessionBefore,
    after: supersessionAfter,
    expectation: {
      kind: 'supersede_restore_command',
      supersededMessageId: 'msg-401',
      winningMessageId: 'msg-402',
      winningMessageKey: 'm-402',
      targetIndex: 2,
    },
  },
] as const satisfies readonly VirtualTranscriptScenario[];

export const virtualTranscriptScenarios = [...scenarios];

export function getVirtualTranscriptScenario(id: VirtualTranscriptScenarioId): VirtualTranscriptScenario {
  const scenario = scenarios.find((item) => item.id === id);
  if (!scenario) {
    throw new Error(`Unknown virtual transcript scenario: ${id}`);
  }
  return scenario;
}

export type { VirtualTranscriptScenarioId } from './types';
