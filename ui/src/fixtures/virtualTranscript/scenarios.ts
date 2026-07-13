import * as v from 'valibot';
import scenarioCorpusJson from '../../../../fixtures/virtual-transcript/v1/scenarios.json';
import type {
  VirtualTranscriptScenario,
  VirtualTranscriptScenarioCorpus,
  VirtualTranscriptScenarioId,
} from './types';

const EXPECTED_SCHEMA_VERSION = 'virtual-transcript.scenarios.v1';
const EXPECTED_SCENARIO_IDS = [
  'prefix-insertion-within-tall-unit',
  'resize-above-anchor',
  'alias-navigation',
  'orphan-target',
  'streaming-growth-reading',
  'streaming-growth-following',
  'supersession',
] as const satisfies readonly VirtualTranscriptScenarioId[];

const UnitRoleSchema = v.picklist(['user', 'agent', 'tool', 'system']);
const StringArraySchema = v.array(v.string());
const NonNegativeNumberSchema = v.pipe(v.number(), v.minValue(0));
const PositiveNumberSchema = v.pipe(v.number(), v.minValue(0), v.notValue(0));
const NonNegativeIntegerSchema = v.pipe(v.number(), v.integer(), v.minValue(0));
const SignedNumberSchema = v.number();

const UnitSchema = v.strictObject({
  key: v.string(),
  role: UnitRoleSchema,
  canonicalMessageId: v.string(),
  aliasMessageIds: StringArraySchema,
  estimatedExtent: NonNegativeNumberSchema,
  measuredExtent: v.optional(NonNegativeNumberSchema),
  text: v.string(),
});

const ViewportSchema = v.strictObject({
  offset: NonNegativeNumberSchema,
  extent: PositiveNumberSchema,
});

const ReadingAnchorSchema = v.strictObject({
  kind: v.literal('message'),
  messageId: v.string(),
});

const VisibleRangeSchema = v.strictObject({
  startIndex: NonNegativeIntegerSchema,
  endIndex: NonNegativeIntegerSchema,
});

const SnapshotSchema = v.strictObject({
  revision: v.string(),
  transcriptGeneration: NonNegativeIntegerSchema,
  units: v.array(UnitSchema),
  viewport: ViewportSchema,
  readingAnchor: v.optional(ReadingAnchorSchema),
  followTail: v.boolean(),
  visibleRange: VisibleRangeSchema,
});

const AliasLookupSchema = v.strictObject({
  requestedMessageId: v.string(),
  resolvedMessageKey: v.nullable(v.string()),
});

const ScenarioSchema = v.variant('id', [
  v.strictObject({
    id: v.literal('prefix-insertion-within-tall-unit'),
    title: v.string(),
    story: v.string(),
    tags: StringArraySchema,
    before: SnapshotSchema,
    after: SnapshotSchema,
    aliasLookups: v.optional(v.array(AliasLookupSchema)),
    expectation: v.strictObject({
      kind: v.literal('restore_anchor_after_prefix_insertion'),
      anchorMessageId: v.string(),
      anchorKey: v.string(),
      previousAnchorOffset: v.number(),
      nextAnchorOffset: v.number(),
      insertedKeys: StringArraySchema,
      preservedViewportDelta: v.number(),
    }),
  }),
  v.strictObject({
    id: v.literal('resize-above-anchor'),
    title: v.string(),
    story: v.string(),
    tags: StringArraySchema,
    before: SnapshotSchema,
    after: SnapshotSchema,
    aliasLookups: v.optional(v.array(AliasLookupSchema)),
    expectation: v.strictObject({
      kind: v.literal('preserve_anchor_across_resize'),
      anchorMessageId: v.string(),
      anchorKey: v.string(),
      previousAnchorOffset: v.number(),
      nextAnchorOffset: v.number(),
      resizedKeys: StringArraySchema,
      preservedViewportDelta: v.number(),
    }),
  }),
  v.strictObject({
    id: v.literal('alias-navigation'),
    title: v.string(),
    story: v.string(),
    tags: StringArraySchema,
    before: SnapshotSchema,
    after: SnapshotSchema,
    aliasLookups: v.optional(v.array(AliasLookupSchema)),
    expectation: v.strictObject({
      kind: v.literal('resolve_alias_navigation'),
      requestedMessageId: v.string(),
      resolvedMessageKey: v.string(),
      targetIndex: NonNegativeIntegerSchema,
      targetOffset: SignedNumberSchema,
    }),
  }),
  v.strictObject({
    id: v.literal('orphan-target'),
    title: v.string(),
    story: v.string(),
    tags: StringArraySchema,
    before: SnapshotSchema,
    after: SnapshotSchema,
    aliasLookups: v.optional(v.array(AliasLookupSchema)),
    expectation: v.strictObject({
      kind: v.literal('report_orphan_target'),
      requestedMessageId: v.string(),
      reason: v.literal('target_missing'),
    }),
  }),
  v.strictObject({
    id: v.literal('streaming-growth-reading'),
    title: v.string(),
    story: v.string(),
    tags: StringArraySchema,
    before: SnapshotSchema,
    after: SnapshotSchema,
    aliasLookups: v.optional(v.array(AliasLookupSchema)),
    expectation: v.strictObject({
      kind: v.literal('stream_append_without_reposition'),
      appendedKeys: StringArraySchema,
      preservedViewportOffset: v.number(),
    }),
  }),
  v.strictObject({
    id: v.literal('streaming-growth-following'),
    title: v.string(),
    story: v.string(),
    tags: StringArraySchema,
    before: SnapshotSchema,
    after: SnapshotSchema,
    aliasLookups: v.optional(v.array(AliasLookupSchema)),
    expectation: v.strictObject({
      kind: v.literal('stream_append_and_follow_tail'),
      appendedKeys: StringArraySchema,
      previousViewportOffset: v.number(),
      nextViewportOffset: v.number(),
      nextViewportEnd: v.number(),
      totalExtent: v.number(),
    }),
  }),
  v.strictObject({
    id: v.literal('supersession'),
    title: v.string(),
    story: v.string(),
    tags: StringArraySchema,
    before: SnapshotSchema,
    after: SnapshotSchema,
    aliasLookups: v.optional(v.array(AliasLookupSchema)),
    expectation: v.strictObject({
      kind: v.literal('supersede_restore_command'),
      supersededMessageId: v.string(),
      winningMessageId: v.string(),
      winningMessageKey: v.string(),
      targetIndex: NonNegativeIntegerSchema,
    }),
  }),
]);

const ScenarioCorpusSchema = v.strictObject({
  schemaVersion: v.literal(EXPECTED_SCHEMA_VERSION),
  metadata: v.strictObject({
    name: v.string(),
    version: v.literal(1),
    unit: v.literal('css_px'),
    scenarioCount: v.literal(7),
  }),
  scenarios: v.array(ScenarioSchema),
});

export function parseVirtualTranscriptScenarioCorpus(raw: unknown): VirtualTranscriptScenarioCorpus {
  const parsed = v.parse(ScenarioCorpusSchema, raw) as VirtualTranscriptScenarioCorpus;
  const ids = parsed.scenarios.map((scenario) => scenario.id);
  const expectedIds = [...EXPECTED_SCENARIO_IDS];

  const duplicateIds = ids.filter((id, index) => ids.indexOf(id) !== index);
  const missingIds = expectedIds.filter((id) => !ids.includes(id));
  const unexpectedIds = ids.filter((id) => !(expectedIds as readonly string[]).includes(id));

  if (duplicateIds.length > 0 || missingIds.length > 0 || unexpectedIds.length > 0) {
    throw new Error(
      `Virtual Transcript fixture IDs invalid: missing [${missingIds.join(', ')}], duplicates [${duplicateIds.join(', ')}], unexpected [${unexpectedIds.join(', ')}]`,
    );
  }

  if (parsed.metadata.scenarioCount !== parsed.scenarios.length) {
    throw new Error(
      `Virtual Transcript fixture metadata scenarioCount ${parsed.metadata.scenarioCount} does not match ${parsed.scenarios.length} scenarios`,
    );
  }

  return parsed;
}

const scenarioCorpus = parseVirtualTranscriptScenarioCorpus(scenarioCorpusJson);
const scenarios = scenarioCorpus.scenarios;

export const virtualTranscriptCorpusMetadata = scenarioCorpus.metadata;
export const virtualTranscriptScenarios: readonly VirtualTranscriptScenario[] = [...scenarios];

export function getVirtualTranscriptScenario(id: VirtualTranscriptScenarioId): VirtualTranscriptScenario {
  const scenario = scenarios.find((item) => item.id === id);
  if (!scenario) {
    throw new Error(`Unknown virtual transcript scenario: ${id}`);
  }
  return scenario;
}

export type { VirtualTranscriptScenarioId } from './types';
