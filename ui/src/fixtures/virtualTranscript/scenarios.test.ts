import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { buildTranscriptLayout } from '../../conversation/virtualTranscriptLayout';
import {
  getVirtualTranscriptScenario,
  parseVirtualTranscriptScenarioCorpus,
  virtualTranscriptCorpusMetadata,
  virtualTranscriptScenarios,
} from './scenarios';
import type {
  VirtualTranscriptScenario,
  VirtualTranscriptScenarioId,
  VirtualTranscriptSnapshot,
  VirtualTranscriptUnit,
} from './types';

const expectedIds: VirtualTranscriptScenarioId[] = [
  'prefix-insertion-within-tall-unit',
  'resize-above-anchor',
  'alias-navigation',
  'orphan-target',
  'streaming-growth-reading',
  'streaming-growth-following',
  'supersession',
];

const fixtureRoot = resolve(__dirname, '../../../../fixtures/virtual-transcript/v1');

interface JsonSchema {
  const?: unknown;
  enum?: unknown[];
  type?: string | string[];
  required?: string[];
  additionalProperties?: boolean;
  properties?: Record<string, JsonSchema>;
  items?: JsonSchema;
  oneOf?: JsonSchema[];
  minimum?: number;
  exclusiveMinimum?: number;
  minItems?: number;
  maxItems?: number;
  $ref?: string;
}

function readFixtureJson(fileName: string): unknown {
  return JSON.parse(readFileSync(resolve(fixtureRoot, fileName), 'utf8'));
}

function resolveRef(schema: JsonSchema, ref: string): JsonSchema {
  const prefix = '#/$defs/';
  if (!ref.startsWith(prefix)) throw new Error(`Unsupported schema ref ${ref}`);
  const defs = (schema as JsonSchema & { $defs?: Record<string, JsonSchema> }).$defs;
  const resolved = defs?.[ref.slice(prefix.length)];
  if (!resolved) throw new Error(`Unknown schema ref ${ref}`);
  return resolved;
}

function validateJsonSchema(value: unknown, rootSchema: JsonSchema, schema: JsonSchema = rootSchema, path = '$'): string[] {
  if (schema.$ref) return validateJsonSchema(value, rootSchema, resolveRef(rootSchema, schema.$ref), path);
  if ('const' in schema && value !== schema.const) return [`${path}: expected const ${JSON.stringify(schema.const)}`];
  if (schema.enum && !schema.enum.includes(value)) return [`${path}: expected one of ${schema.enum.join(', ')}`];

  const allowedTypes = Array.isArray(schema.type) ? schema.type : schema.type ? [schema.type] : [];
  if (allowedTypes.length > 0) {
    const actualType = value === null ? 'null' : Array.isArray(value) ? 'array' : Number.isInteger(value) ? 'integer' : typeof value;
    if (!allowedTypes.some((type) => type === actualType || (type === 'number' && actualType === 'integer'))) {
      return [`${path}: expected type ${allowedTypes.join(' | ')}, got ${actualType}`];
    }
  }

  if (typeof value === 'number') {
    if (schema.minimum !== undefined && value < schema.minimum) return [`${path}: expected >= ${schema.minimum}`];
    if (schema.exclusiveMinimum !== undefined && value <= schema.exclusiveMinimum) return [`${path}: expected > ${schema.exclusiveMinimum}`];
  }

  if (Array.isArray(value)) {
    const errors: string[] = [];
    if (schema.minItems !== undefined && value.length < schema.minItems) errors.push(`${path}: expected at least ${schema.minItems} items`);
    if (schema.maxItems !== undefined && value.length > schema.maxItems) errors.push(`${path}: expected at most ${schema.maxItems} items`);
    if (schema.items) {
      value.forEach((item, index) => errors.push(...validateJsonSchema(item, rootSchema, schema.items!, `${path}[${index}]`)));
    }
    return errors;
  }

  if (schema.oneOf) {
    const matches = schema.oneOf.filter((option) => validateJsonSchema(value, rootSchema, option, path).length === 0);
    return matches.length === 1 ? [] : [`${path}: expected exactly one matching oneOf branch, got ${matches.length}`];
  }

  if (schema.properties || schema.required || schema.additionalProperties === false) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return [`${path}: expected object`];
    const record = value as Record<string, unknown>;
    const errors: string[] = [];
    for (const key of schema.required ?? []) {
      if (!(key in record)) errors.push(`${path}.${key}: missing required property`);
    }
    for (const [key, childValue] of Object.entries(record)) {
      const childSchema = schema.properties?.[key];
      if (!childSchema) {
        if (schema.additionalProperties === false) errors.push(`${path}.${key}: unexpected property`);
        continue;
      }
      errors.push(...validateJsonSchema(childValue, rootSchema, childSchema, `${path}.${key}`));
    }
    return errors;
  }

  return [];
}

function layoutFor(units: readonly VirtualTranscriptUnit[]) {
  return buildTranscriptLayout({
    keys: units.map((unit) => unit.key),
    estimatedExtent: (_key, index) => units[index]?.estimatedExtent ?? 0,
    measuredExtents: new Map(
      units.flatMap((unit) => unit.measuredExtent === undefined ? [] : [[unit.key, unit.measuredExtent] as const]),
    ),
  });
}

function findUnit(snapshot: VirtualTranscriptSnapshot, messageId: string) {
  return snapshot.units.find(
    (unit) => unit.canonicalMessageId === messageId || unit.aliasMessageIds?.includes(messageId),
  );
}

function offsetFor(snapshot: VirtualTranscriptSnapshot, messageId: string) {
  const unit = findUnit(snapshot, messageId);
  expect(unit).toBeDefined();
  return layoutFor(snapshot.units).offsetForKey(unit!.key);
}

function appendedKeys(before: VirtualTranscriptSnapshot, after: VirtualTranscriptSnapshot) {
  const beforeKeys = new Set(before.units.map((unit) => unit.key));
  return after.units.map((unit) => unit.key).filter((key) => !beforeKeys.has(key));
}

function assertVisibleRange(snapshot: VirtualTranscriptSnapshot) {
  const layout = layoutFor(snapshot.units);
  expect(snapshot.visibleRange).toEqual(
    layout.rangeForViewport({
      viewportOffset: snapshot.viewport.offset,
      viewportExtent: snapshot.viewport.extent,
      overscanExtent: 0,
    }),
  );
}

function assertScenarioExpectation(scenario: VirtualTranscriptScenario) {
  const expectation = scenario.expectation;
  switch (expectation.kind) {
    case 'restore_anchor_after_prefix_insertion': {
      expect(offsetFor(scenario.before, expectation.anchorMessageId)).toBe(expectation.previousAnchorOffset);
      expect(offsetFor(scenario.after, expectation.anchorMessageId)).toBe(expectation.nextAnchorOffset);
      expect(appendedKeys(scenario.before, scenario.after)).toEqual(expectation.insertedKeys);
      expect(expectation.nextAnchorOffset - expectation.previousAnchorOffset).toBe(expectation.preservedViewportDelta);
      expect(scenario.after.viewport.offset - scenario.before.viewport.offset).toBe(expectation.preservedViewportDelta);
      expect(expectation.previousAnchorOffset - scenario.before.viewport.offset)
        .toBe(expectation.nextAnchorOffset - scenario.after.viewport.offset);
      return;
    }

    case 'preserve_anchor_across_resize': {
      expect(offsetFor(scenario.before, expectation.anchorMessageId)).toBe(expectation.previousAnchorOffset);
      expect(offsetFor(scenario.after, expectation.anchorMessageId)).toBe(expectation.nextAnchorOffset);
      expect(expectation.nextAnchorOffset - expectation.previousAnchorOffset).toBe(expectation.preservedViewportDelta);
      expect(scenario.after.viewport.offset - scenario.before.viewport.offset).toBe(expectation.preservedViewportDelta);
      expect(expectation.previousAnchorOffset - scenario.before.viewport.offset)
        .toBe(expectation.nextAnchorOffset - scenario.after.viewport.offset);
      const changed = scenario.after.units
        .filter((afterUnit) => {
          const beforeUnit = scenario.before.units.find((unit) => unit.key === afterUnit.key);
          return beforeUnit && beforeUnit.measuredExtent !== afterUnit.measuredExtent;
        })
        .map((unit) => unit.key);
      expect(changed).toEqual(expectation.resizedKeys);
      return;
    }

    case 'resolve_alias_navigation': {
      expect(findUnit(scenario.after, expectation.requestedMessageId)?.key).toBe(expectation.resolvedMessageKey);
      expect(offsetFor(scenario.after, expectation.requestedMessageId)).toBe(expectation.targetOffset);
      expect(layoutFor(scenario.after.units).indexForKey(expectation.resolvedMessageKey)).toBe(expectation.targetIndex);
      expect(scenario.aliasLookups?.some((lookup) => lookup.requestedMessageId === expectation.requestedMessageId && lookup.resolvedMessageKey === expectation.resolvedMessageKey)).toBe(true);
      return;
    }

    case 'report_orphan_target': {
      expect(findUnit(scenario.after, expectation.requestedMessageId)).toBeUndefined();
      expect(scenario.aliasLookups).toEqual([
        { requestedMessageId: expectation.requestedMessageId, resolvedMessageKey: null },
      ]);
      return;
    }

    case 'stream_append_without_reposition': {
      expect(appendedKeys(scenario.before, scenario.after)).toEqual(expectation.appendedKeys);
      expect(scenario.before.viewport.offset).toBe(expectation.preservedViewportOffset);
      expect(scenario.after.viewport.offset).toBe(expectation.preservedViewportOffset);
      return;
    }

    case 'stream_append_and_follow_tail': {
      const afterLayout = layoutFor(scenario.after.units);
      expect(appendedKeys(scenario.before, scenario.after)).toEqual(expectation.appendedKeys);
      expect(scenario.before.viewport.offset).toBe(expectation.previousViewportOffset);
      expect(scenario.after.viewport.offset).toBe(expectation.nextViewportOffset);
      expect(expectation.nextViewportEnd).toBe(expectation.nextViewportOffset + scenario.after.viewport.extent);
      expect(afterLayout.totalExtent).toBe(expectation.totalExtent);
      expect(expectation.nextViewportEnd).toBe(expectation.totalExtent);
      return;
    }

    case 'supersede_restore_command': {
      expect(findUnit(scenario.after, expectation.supersededMessageId)?.key).not.toBe(expectation.winningMessageKey);
      expect(findUnit(scenario.after, expectation.winningMessageId)?.key).toBe(expectation.winningMessageKey);
      expect(layoutFor(scenario.after.units).indexForKey(expectation.winningMessageKey)).toBe(expectation.targetIndex);
      expect(scenario.before.readingAnchor?.messageId).toBe(expectation.supersededMessageId);
      expect(scenario.after.readingAnchor?.messageId).toBe(expectation.winningMessageId);
      return;
    }
  }
}

describe('virtual transcript fixture scenarios', () => {
  it('ships portable JSON that validates against the draft 2020-12 schema', () => {
    const schema = readFixtureJson('schema.json') as JsonSchema;
    const corpus = readFixtureJson('scenarios.json');

    expect((schema as JsonSchema & { $schema?: string }).$schema).toBe('https://json-schema.org/draft/2020-12/schema');
    expect(validateJsonSchema(corpus, schema)).toEqual([]);
    expect(virtualTranscriptCorpusMetadata).toEqual({
      name: 'Virtual Transcript conformance corpus',
      version: 1,
      unit: 'css_px',
      scenarioCount: expectedIds.length,
    });
  });

  it('rejects a corpus with the wrong schema version or scenario ids', () => {
    const corpus = readFixtureJson('scenarios.json') as Record<string, unknown>;

    expect(() => parseVirtualTranscriptScenarioCorpus({ ...corpus, schemaVersion: 'virtual-transcript.scenarios.v2' }))
      .toThrow();

    const scenarios = [...(corpus['scenarios'] as Record<string, unknown>[])];
    scenarios[0] = { ...scenarios[0], id: 'unexpected-id' };
    expect(() => parseVirtualTranscriptScenarioCorpus({ ...corpus, scenarios })).toThrow();
  });

  it('covers the intended conformance corpus with stable ids and lookups', () => {
    expect(virtualTranscriptScenarios.map((scenario) => scenario.id)).toEqual(expectedIds);
    expect(new Set(expectedIds).size).toBe(expectedIds.length);

    for (const id of expectedIds) {
      expect(getVirtualTranscriptScenario(id).id).toBe(id);
    }
  });

  it('keeps every snapshot internally consistent with transcript layout math', () => {
    for (const scenario of virtualTranscriptScenarios) {
      assertVisibleRange(scenario.before);
      assertVisibleRange(scenario.after);

      for (const snapshot of [scenario.before, scenario.after]) {
        const keys = snapshot.units.map((unit) => unit.key);
        expect(new Set(keys).size).toBe(keys.length);
        expect(snapshot.viewport.extent).toBeGreaterThan(0);
        expect(snapshot.viewport.offset).toBeGreaterThanOrEqual(0);
        if (snapshot.readingAnchor) {
          expect(findUnit(snapshot, snapshot.readingAnchor.messageId)).toBeDefined();
        }
      }
    }
  });

  it('encodes each requested behavior as a deterministic expectation', () => {
    for (const scenario of virtualTranscriptScenarios) {
      assertScenarioExpectation(scenario);
    }
  });

  it('keeps transcript generations monotonic for growth scenarios and stable for pure reflow', () => {
    const byId = new Map(virtualTranscriptScenarios.map((scenario) => [scenario.id, scenario]));

    expect(byId.get('prefix-insertion-within-tall-unit')?.after.transcriptGeneration)
      .toBeGreaterThan(byId.get('prefix-insertion-within-tall-unit')!.before.transcriptGeneration);
    expect(byId.get('streaming-growth-reading')?.after.transcriptGeneration)
      .toBeGreaterThan(byId.get('streaming-growth-reading')!.before.transcriptGeneration);
    expect(byId.get('streaming-growth-following')?.after.transcriptGeneration)
      .toBeGreaterThan(byId.get('streaming-growth-following')!.before.transcriptGeneration);

    expect(byId.get('resize-above-anchor')?.after.transcriptGeneration)
      .toBe(byId.get('resize-above-anchor')!.before.transcriptGeneration);
    expect(byId.get('alias-navigation')?.after.transcriptGeneration)
      .toBe(byId.get('alias-navigation')!.before.transcriptGeneration);
    expect(byId.get('orphan-target')?.after.transcriptGeneration)
      .toBe(byId.get('orphan-target')!.before.transcriptGeneration);
    expect(byId.get('supersession')?.after.transcriptGeneration)
      .toBe(byId.get('supersession')!.before.transcriptGeneration);
  });
});
