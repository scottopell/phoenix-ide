import { describe, expect, it } from 'vitest';

import type { HistoryExpansionState, HistoryScrollCommand, HistoryView } from './historyExpansion';
import {
  initialTranscriptPositioningState,
  reduceTranscriptPositioning,
  transcriptPositioningCommandKey,
  transcriptPositioningInputFromHistoryExpansion,
  type TranscriptPositioningEffect,
  type TranscriptPositioningState,
} from './transcriptPositioning';

const view: HistoryView = { conversationId: 'conv', generation: 1, transcriptGeneration: 1 };
const nextView: HistoryView = { conversationId: 'conv', generation: 1, transcriptGeneration: 2 };

function restore(overrides: Partial<Extract<HistoryScrollCommand, { kind: 'restore_after_prefix_expansion' }>> = {}): Extract<HistoryScrollCommand, { kind: 'restore_after_prefix_expansion' }> {
  return {
    kind: 'restore_after_prefix_expansion',
    token: 1,
    requestToken: 10,
    view,
    messageId: 'msg-2',
    viewportStartOffset: 24,
    ...overrides,
  };
}

function jump(overrides: Partial<Extract<HistoryScrollCommand, { kind: 'jump_to_message' }>> = {}): Extract<HistoryScrollCommand, { kind: 'jump_to_message' }> {
  return {
    kind: 'jump_to_message',
    token: 1,
    requestToken: 10,
    view,
    targetMessageId: 'msg-2',
    ...overrides,
  };
}

function apply(
  state: TranscriptPositioningState,
  event: Parameters<typeof reduceTranscriptPositioning>[1],
): [TranscriptPositioningState, TranscriptPositioningEffect[]] {
  const reduced = reduceTranscriptPositioning(state, event);
  return [reduced.state, reduced.effects];
}

function start(command: HistoryScrollCommand, state = initialTranscriptPositioningState(view)): [TranscriptPositioningState, TranscriptPositioningEffect[]] {
  return apply(state, { type: 'input_changed', input: { kind: 'positioning', command } });
}

function resolveAndIssue(
  command: HistoryScrollCommand,
  targetIndex = 1,
  layoutRevision = 5,
): TranscriptPositioningState {
  let [state] = start(command);
  [state] = apply(state, { type: 'target_resolved', commandKey: transcriptPositioningCommandKey(command), targetIndex });
  [state] = apply(state, {
    type: 'position_issued',
    commandKey: transcriptPositioningCommandKey(command),
    targetIndex,
    layoutRevision,
  });
  return state;
}

describe('transcript positioning reducer', () => {
  it('derives the closed parent input from HistoryExpansionState', () => {
    const command = restore();
    const base: HistoryExpansionState = {
      view,
      coverage: 'complete',
      activeRequest: null,
      pendingCommand: null,
      failure: null,
    };

    expect(transcriptPositioningInputFromHistoryExpansion(base)).toEqual({ kind: 'idle', view });
    expect(transcriptPositioningInputFromHistoryExpansion({ ...base, pendingCommand: command })).toEqual({
      kind: 'positioning',
      command,
    });
  });

  it('starts by resolving the command target and does not reactivate an identical input', () => {
    const command = restore();
    let [state, effects] = start(command);

    expect(state.currentView).toBe(view);
    expect(state.active?.key).toBe(transcriptPositioningCommandKey(command));
    expect(state.phase).toEqual({ kind: 'resolving_target' });
    expect(effects).toEqual([{ type: 'resolve_target', command, commandKey: transcriptPositioningCommandKey(command), targetMessageId: 'msg-2' }]);

    [state, effects] = apply(state, { type: 'input_changed', input: { kind: 'positioning', command } });
    expect(effects).toEqual([]);
    expect(state.active?.command).toBe(command);
  });

  it('finishes a prior command before resolving a replacement in exact effect order', () => {
    const first = restore({ token: 1, messageId: 'msg-2' });
    const second = restore({ token: 2, messageId: 'msg-3' });
    let [state] = start(first);

    const reduced = reduceTranscriptPositioning(state, { type: 'input_changed', input: { kind: 'positioning', command: second } });
    state = reduced.state;

    expect(reduced.effects).toEqual([
      { type: 'finish', command: first, commandKey: transcriptPositioningCommandKey(first), result: 'superseded' },
      { type: 'resolve_target', command: second, commandKey: transcriptPositioningCommandKey(second), targetMessageId: 'msg-3' },
    ]);
    expect(state.active?.command).toBe(second);
    expect(state.terminalCommandKeys.has(transcriptPositioningCommandKey(first))).toBe(true);
  });

  it('finishes an active command when input becomes idle or the view changes, without resolving anything new', () => {
    const command = restore();
    let [state] = start(command);
    let effects: TranscriptPositioningEffect[];

    [state, effects] = apply(state, { type: 'input_changed', input: { kind: 'idle', view } });
    expect(effects).toEqual([{ type: 'finish', command, commandKey: transcriptPositioningCommandKey(command), result: 'superseded' }]);
    expect(state.active).toBeNull();
    expect(state.currentView).toBe(view);

    [state, effects] = start(restore({ token: 2 }), state);
    expect(effects[0]?.type).toBe('resolve_target');
    [state, effects] = apply(state, { type: 'input_changed', input: { kind: 'idle', view: nextView } });
    expect(effects).toEqual([{ type: 'finish', command: restore({ token: 2 }), commandKey: transcriptPositioningCommandKey(restore({ token: 2 })), result: 'superseded' }]);
    expect(state.currentView).toBe(nextView);
  });

  it('never reactivates a terminal command identity or finishes it twice', () => {
    const command = restore();
    const key = transcriptPositioningCommandKey(command);
    let [state] = start(command);
    let effects: TranscriptPositioningEffect[];

    [state, effects] = apply(state, { type: 'user_interrupted' });
    expect(effects).toEqual([{ type: 'finish', command, commandKey: key, result: 'superseded' }]);

    for (const event of [
      { type: 'user_interrupted' as const },
      { type: 'executor_detached' as const },
      { type: 'target_missing' as const, commandKey: key },
      { type: 'target_resolved' as const, commandKey: key, targetIndex: 1 },
      { type: 'input_changed' as const, input: { kind: 'positioning' as const, command } },
    ]) {
      [state, effects] = apply(state, event);
      expect(effects).toEqual([]);
      expect(state.active).toBeNull();
    }
  });

  it('ignores stale target and physical events after replacement', () => {
    const first = jump({ token: 1, targetMessageId: 'msg-2' });
    const second = jump({ token: 2, targetMessageId: 'msg-3' });
    const firstKey = transcriptPositioningCommandKey(first);
    const secondKey = transcriptPositioningCommandKey(second);
    let [state] = start(first);
    [state] = apply(state, { type: 'input_changed', input: { kind: 'positioning', command: second } });

    let effects: TranscriptPositioningEffect[];
    [state, effects] = apply(state, { type: 'target_resolved', commandKey: firstKey, targetIndex: 1 });
    expect(effects).toEqual([]);
    expect(state.phase).toEqual({ kind: 'resolving_target' });

    [state, effects] = apply(state, { type: 'target_resolved', commandKey: secondKey, targetIndex: 2 });
    expect(effects).toEqual([{ type: 'position', command: second, commandKey: secondKey, targetIndex: 2, align: 'start' }]);

    [state, effects] = apply(state, { type: 'position_issued', commandKey: firstKey, targetIndex: 2, layoutRevision: 7 });
    expect(effects).toEqual([]);
    [state, effects] = apply(state, { type: 'physical_observed', commandKey: firstKey, range: { startIndex: 2, endIndex: 2 }, actualOffset: null, layoutRevision: 7, targetMeasured: true });
    expect(effects).toEqual([]);
    expect(state.active?.command).toBe(second);
  });

  it('ignores stale position_issued events for the wrong target or an older revision', () => {
    const command = restore();
    const key = transcriptPositioningCommandKey(command);
    let [state] = start(command);
    let effects: TranscriptPositioningEffect[];

    [state] = apply(state, { type: 'target_resolved', commandKey: key, targetIndex: 2 });
    [state, effects] = apply(state, { type: 'position_issued', commandKey: key, targetIndex: 1, layoutRevision: 10 });
    expect(effects).toEqual([]);
    expect(state.phase).toEqual({ kind: 'awaiting_physical', targetIndex: 2, issuedLayoutRevision: Number.POSITIVE_INFINITY });

    [state] = apply(state, { type: 'position_issued', commandKey: key, targetIndex: 2, layoutRevision: 10 });
    [state, effects] = apply(state, { type: 'position_issued', commandKey: key, targetIndex: 2, layoutRevision: 9 });
    expect(effects).toEqual([]);
    expect(state.phase).toEqual({ kind: 'awaiting_physical', targetIndex: 2, issuedLayoutRevision: 10 });
  });

  it('emits target_missing only while resolving and ignores duplicate missing notifications', () => {
    const command = jump();
    const key = transcriptPositioningCommandKey(command);
    let [state] = start(command);
    let effects: TranscriptPositioningEffect[];

    [state, effects] = apply(state, { type: 'target_missing', commandKey: key });
    expect(effects).toEqual([{ type: 'finish', command, commandKey: key, result: 'target_missing' }]);
    expect(state.active).toBeNull();

    [state, effects] = apply(state, { type: 'target_missing', commandKey: key });
    expect(effects).toEqual([]);
  });

  it('positions restores with the signed viewport offset and waits for the issued layout revision', () => {
    const command = restore({ viewportStartOffset: 24 });
    const key = transcriptPositioningCommandKey(command);
    let [state] = start(command);
    let effects: TranscriptPositioningEffect[];

    [state, effects] = apply(state, { type: 'target_resolved', commandKey: key, targetIndex: 1 });
    expect(effects).toEqual([{ type: 'position', command, commandKey: key, targetIndex: 1, align: 'start', viewportStartOffset: 24 }]);

    [state, effects] = apply(state, { type: 'physical_observed', commandKey: key, range: { startIndex: 1, endIndex: 1 }, actualOffset: 24, layoutRevision: 99, targetMeasured: true });
    expect(effects).toEqual([]);

    [state] = apply(state, { type: 'position_issued', commandKey: key, targetIndex: 1, layoutRevision: 5 });
    [state, effects] = apply(state, { type: 'physical_observed', commandKey: key, range: { startIndex: 1, endIndex: 1 }, actualOffset: 24, layoutRevision: 4, targetMeasured: true });
    expect(effects).toEqual([]);
    expect(state.active?.command).toBe(command);
  });

  it('applies restore only when visible, within 2px, and observed at or after the issued revision', () => {
    const command = restore({ viewportStartOffset: 24 });
    const key = transcriptPositioningCommandKey(command);
    const nonTerminalObservations = [
      { range: null, actualOffset: 24, layoutRevision: 5, targetMeasured: true },
      { range: { startIndex: 0, endIndex: 0 }, actualOffset: 24, layoutRevision: 5, targetMeasured: true },
      { range: { startIndex: 1, endIndex: 1 }, actualOffset: null, layoutRevision: 5, targetMeasured: true },
      { range: { startIndex: 1, endIndex: 1 }, actualOffset: 21.99, layoutRevision: 5, targetMeasured: true },
      { range: { startIndex: 1, endIndex: 1 }, actualOffset: 24, layoutRevision: 4, targetMeasured: true },
    ];

    for (const observation of nonTerminalObservations) {
      const state = resolveAndIssue(command, 1, 5);
      const [nextState, effects] = apply(state, { type: 'physical_observed', commandKey: key, ...observation, targetMeasured: true });
      expect(effects).toEqual([]);
      expect(nextState.active?.command).toBe(command);
    }

    let state = resolveAndIssue(command, 1, 5);
    const [nextState, effects] = apply(state, { type: 'physical_observed', commandKey: key, range: { startIndex: 1, endIndex: 1 }, actualOffset: 22, layoutRevision: 5, targetMeasured: true });
    state = nextState;
    expect(effects).toEqual([{ type: 'finish', command, commandKey: key, result: 'applied' }]);
    expect(state.active).toBeNull();
  });

  it('applies jumps when target is visible regardless of offset, after issued revision', () => {
    const command = jump();
    const key = transcriptPositioningCommandKey(command);
    let state = resolveAndIssue(command, 1, 5);
    let effects: TranscriptPositioningEffect[];

    [state, effects] = apply(state, { type: 'physical_observed', commandKey: key, range: { startIndex: 0, endIndex: 0 }, actualOffset: null, layoutRevision: 5, targetMeasured: true });
    expect(effects).toEqual([]);

    [state, effects] = apply(state, { type: 'physical_observed', commandKey: key, range: { startIndex: 1, endIndex: 1 }, actualOffset: 1000, layoutRevision: 5, targetMeasured: true });
    expect(effects).toEqual([{ type: 'finish', command, commandKey: key, result: 'applied' }]);
  });

  it.each(['user_interrupted', 'executor_detached'] as const)('%s supersedes resolving and awaiting commands', (type) => {
    const resolving = restore({ token: 1 });
    let [state] = start(resolving);
    let effects: TranscriptPositioningEffect[];

    [state, effects] = apply(state, { type });
    expect(effects).toEqual([{ type: 'finish', command: resolving, commandKey: transcriptPositioningCommandKey(resolving), result: 'superseded' }]);
    expect(state.active).toBeNull();

    const awaiting = restore({ token: 2 });
    state = resolveAndIssue(awaiting, 1, 5);
    [state, effects] = apply(state, { type });
    expect(effects).toEqual([{ type: 'finish', command: awaiting, commandKey: transcriptPositioningCommandKey(awaiting), result: 'superseded' }]);
    expect(state.active).toBeNull();
  });
});
