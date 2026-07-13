export type VirtualTranscriptScenarioId =
  | 'prefix-insertion-within-tall-unit'
  | 'resize-above-anchor'
  | 'alias-navigation'
  | 'orphan-target'
  | 'streaming-growth-reading'
  | 'streaming-growth-following'
  | 'supersession';

export interface VirtualTranscriptCorpusMetadata {
  name: string;
  version: 1;
  unit: 'css_px';
  scenarioCount: number;
}

export interface VirtualTranscriptScenarioCorpus {
  schemaVersion: 'virtual-transcript.scenarios.v1';
  metadata: VirtualTranscriptCorpusMetadata;
  scenarios: readonly VirtualTranscriptScenario[];
}

export type VirtualTranscriptUnitRole = 'user' | 'agent' | 'tool' | 'system';

export interface VirtualTranscriptUnit {
  key: string;
  role: VirtualTranscriptUnitRole;
  canonicalMessageId: string;
  aliasMessageIds: readonly string[];
  estimatedExtent: number;
  measuredExtent?: number;
  text: string;
}

export interface VirtualTranscriptViewport {
  offset: number;
  extent: number;
}

export interface VirtualTranscriptAnchor {
  kind: 'message';
  messageId: string;
}

export interface VirtualTranscriptAliasLookup {
  requestedMessageId: string;
  resolvedMessageKey: string | null;
}

export interface VirtualTranscriptVisibleRange {
  startIndex: number;
  endIndex: number;
}

export interface VirtualTranscriptSnapshot {
  revision: string;
  transcriptGeneration: number;
  units: readonly VirtualTranscriptUnit[];
  viewport: VirtualTranscriptViewport;
  readingAnchor?: VirtualTranscriptAnchor;
  followTail: boolean;
  visibleRange: VirtualTranscriptVisibleRange;
}

export type VirtualTranscriptExpectation =
  | {
      kind: 'restore_anchor_after_prefix_insertion';
      anchorMessageId: string;
      anchorKey: string;
      previousAnchorOffset: number;
      nextAnchorOffset: number;
      insertedKeys: readonly string[];
      preservedViewportDelta: number;
    }
  | {
      kind: 'preserve_anchor_across_resize';
      anchorMessageId: string;
      anchorKey: string;
      previousAnchorOffset: number;
      nextAnchorOffset: number;
      resizedKeys: readonly string[];
      preservedViewportDelta: number;
    }
  | {
      kind: 'resolve_alias_navigation';
      requestedMessageId: string;
      resolvedMessageKey: string;
      targetIndex: number;
      targetOffset: number;
    }
  | {
      kind: 'report_orphan_target';
      requestedMessageId: string;
      reason: 'target_missing';
    }
  | {
      kind: 'stream_append_without_reposition';
      appendedKeys: readonly string[];
      preservedViewportOffset: number;
    }
  | {
      kind: 'stream_append_and_follow_tail';
      appendedKeys: readonly string[];
      previousViewportOffset: number;
      nextViewportOffset: number;
      nextViewportEnd: number;
      totalExtent: number;
    }
  | {
      kind: 'supersede_restore_command';
      supersededMessageId: string;
      winningMessageId: string;
      winningMessageKey: string;
      targetIndex: number;
    };

export interface VirtualTranscriptScenario {
  id: VirtualTranscriptScenarioId;
  title: string;
  story: string;
  tags: readonly string[];
  before: VirtualTranscriptSnapshot;
  after: VirtualTranscriptSnapshot;
  aliasLookups?: readonly VirtualTranscriptAliasLookup[];
  expectation: VirtualTranscriptExpectation;
}
