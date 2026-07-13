import type { HistoryExpansionState, HistoryScrollCommand, HistoryView } from './historyExpansion';

export type TranscriptPositioningInput =
  | { kind: 'idle'; view: HistoryView }
  | { kind: 'positioning'; command: HistoryScrollCommand };

export type TranscriptPositioningFinishResult = 'applied' | 'target_missing' | 'superseded';

export type TranscriptVisibleRange = {
  startIndex: number;
  endIndex: number;
};

export type TranscriptPositioningPhase =
  | { kind: 'resolving_target' }
  | { kind: 'awaiting_physical'; targetIndex: number; issuedLayoutRevision: number };

export type ActiveTranscriptPositioningCommand = {
  command: HistoryScrollCommand;
  key: string;
};

export type TranscriptPositioningState = {
  currentView: HistoryView | null;
  active: ActiveTranscriptPositioningCommand | null;
  phase: TranscriptPositioningPhase | null;
  terminalCommandKeys: ReadonlySet<string>;
};

export type TranscriptPositioningEvent =
  | { type: 'input_changed'; input: TranscriptPositioningInput }
  | { type: 'target_resolved'; commandKey: string; targetIndex: number }
  | { type: 'target_missing'; commandKey: string }
  | { type: 'position_issued'; commandKey: string; targetIndex: number; layoutRevision: number }
  | {
      type: 'physical_observed';
      commandKey: string;
      range: TranscriptVisibleRange | null;
      actualOffset: number | null;
      layoutRevision: number;
      targetMeasured: boolean;
    }
  | { type: 'user_interrupted' }
  | { type: 'executor_detached' };

export type TranscriptPositioningEffect =
  | { type: 'resolve_target'; command: HistoryScrollCommand; commandKey: string; targetMessageId: string }
  | {
      type: 'position';
      command: HistoryScrollCommand;
      commandKey: string;
      targetIndex: number;
      align: 'start';
      viewportStartOffset?: number;
    }
  | {
      type: 'finish';
      command: HistoryScrollCommand;
      commandKey: string;
      result: TranscriptPositioningFinishResult;
    };

export type TranscriptPositioningReduction = {
  state: TranscriptPositioningState;
  effects: TranscriptPositioningEffect[];
};

const RESTORE_OFFSET_TOLERANCE_PX = 2;
const UNISSUED_LAYOUT_REVISION = Number.POSITIVE_INFINITY;

export function initialTranscriptPositioningState(view: HistoryView | null = null): TranscriptPositioningState {
  return {
    currentView: view,
    active: null,
    phase: null,
    terminalCommandKeys: new Set(),
  };
}

export function transcriptPositioningInputFromHistoryExpansion(
  history: HistoryExpansionState,
): TranscriptPositioningInput {
  return history.pendingCommand
    ? { kind: 'positioning', command: history.pendingCommand }
    : { kind: 'idle', view: history.view };
}

export function transcriptPositioningCommandKey(command: HistoryScrollCommand): string {
  return [
    command.kind,
    command.token,
    command.requestToken,
    command.view.conversationId,
    command.view.generation,
    command.view.transcriptGeneration,
    command.kind === 'jump_to_message' ? command.targetMessageId : command.messageId,
  ].join(':');
}

export function sameHistoryView(left: HistoryView, right: HistoryView): boolean {
  return left.conversationId === right.conversationId
    && left.generation === right.generation
    && left.transcriptGeneration === right.transcriptGeneration;
}

export function reduceTranscriptPositioning(
  state: TranscriptPositioningState,
  event: TranscriptPositioningEvent,
): TranscriptPositioningReduction {
  switch (event.type) {
    case 'input_changed':
      return reduceInputChanged(state, event.input);

    case 'target_resolved': {
      if (!isCurrentResolvingEvent(state, event.commandKey) || state.terminalCommandKeys.has(event.commandKey)) {
        return unchanged(state);
      }
      return {
        state: {
          ...state,
          phase: {
            kind: 'awaiting_physical',
            targetIndex: event.targetIndex,
            issuedLayoutRevision: UNISSUED_LAYOUT_REVISION,
          },
        },
        effects: [positionEffect(state.active.command, event.commandKey, event.targetIndex)],
      };
    }

    case 'target_missing': {
      if (!isCurrentResolvingEvent(state, event.commandKey)) return unchanged(state);
      return finishActive(state, 'target_missing');
    }

    case 'position_issued': {
      if (!isCurrentAwaitingEvent(state, event.commandKey, event.targetIndex)
        || (state.phase.issuedLayoutRevision !== UNISSUED_LAYOUT_REVISION
          && event.layoutRevision < state.phase.issuedLayoutRevision)) {
        return unchanged(state);
      }
      return {
        state: {
          ...state,
          phase: { ...state.phase, issuedLayoutRevision: event.layoutRevision },
        },
        effects: [],
      };
    }

    case 'physical_observed': {
      if (!state.active || state.active.key !== event.commandKey || state.phase?.kind !== 'awaiting_physical') {
        return unchanged(state);
      }
      if (state.phase.issuedLayoutRevision === UNISSUED_LAYOUT_REVISION
        || event.layoutRevision < state.phase.issuedLayoutRevision
        || !rangeContains(event.range, state.phase.targetIndex)
        || !event.targetMeasured) {
        return unchanged(state);
      }
      if (state.active.command.kind === 'jump_to_message') {
        return finishActive(state, 'applied');
      }
      if (event.actualOffset === null) return unchanged(state);
      const expectedOffset = state.active.command.viewportStartOffset;
      if (Math.abs(event.actualOffset - expectedOffset) > RESTORE_OFFSET_TOLERANCE_PX) {
        return unchanged(state);
      }
      return finishActive(state, 'applied');
    }

    case 'user_interrupted':
    case 'executor_detached':
      return finishActive(state, 'superseded');
  }
}

function reduceInputChanged(
  state: TranscriptPositioningState,
  input: TranscriptPositioningInput,
): TranscriptPositioningReduction {
  const effects: TranscriptPositioningEffect[] = [];
  let nextState = state;
  const nextView = input.kind === 'idle' ? input.view : input.command.view;
  const nextKey = input.kind === 'positioning' ? transcriptPositioningCommandKey(input.command) : null;

  if (state.active) {
    const sameActive = input.kind === 'positioning'
      && state.active.key === nextKey
      && sameHistoryView(state.active.command.view, input.command.view);
    if (sameActive) return unchanged(state);
    const finished = finishActive(nextState, 'superseded');
    nextState = finished.state;
    effects.push(...finished.effects);
  }

  const viewChanged = nextState.currentView !== null && !sameHistoryView(nextState.currentView, nextView);
  nextState = {
    ...nextState,
    currentView: nextView,
    terminalCommandKeys: viewChanged ? new Set() : nextState.terminalCommandKeys,
  };

  if (input.kind === 'idle') {
    return { state: nextState, effects };
  }

  const key = nextKey ?? transcriptPositioningCommandKey(input.command);
  if (nextState.terminalCommandKeys.has(key)) {
    return { state: nextState, effects };
  }

  nextState = {
    ...nextState,
    active: { command: input.command, key },
    phase: { kind: 'resolving_target' },
  };
  effects.push({
    type: 'resolve_target',
    command: input.command,
    commandKey: key,
    targetMessageId: targetMessageId(input.command),
  });
  return { state: nextState, effects };
}

function finishActive(
  state: TranscriptPositioningState,
  result: TranscriptPositioningFinishResult,
): TranscriptPositioningReduction {
  if (!state.active || state.terminalCommandKeys.has(state.active.key)) return unchanged(state);
  const terminalCommandKeys = new Set(state.terminalCommandKeys);
  terminalCommandKeys.add(state.active.key);
  return {
    state: {
      ...state,
      active: null,
      phase: null,
      terminalCommandKeys,
    },
    effects: [{
      type: 'finish',
      command: state.active.command,
      commandKey: state.active.key,
      result,
    }],
  };
}

function unchanged(state: TranscriptPositioningState): TranscriptPositioningReduction {
  return { state, effects: [] };
}

function isCurrentResolvingEvent(state: TranscriptPositioningState, commandKey: string): state is TranscriptPositioningState & {
  active: ActiveTranscriptPositioningCommand;
  phase: { kind: 'resolving_target' };
} {
  return state.active?.key === commandKey && state.phase?.kind === 'resolving_target';
}

function isCurrentAwaitingEvent(
  state: TranscriptPositioningState,
  commandKey: string,
  targetIndex: number,
): state is TranscriptPositioningState & {
  active: ActiveTranscriptPositioningCommand;
  phase: { kind: 'awaiting_physical'; targetIndex: number; issuedLayoutRevision: number };
} {
  return state.active?.key === commandKey
    && state.phase?.kind === 'awaiting_physical'
    && state.phase.targetIndex === targetIndex;
}

function positionEffect(
  command: HistoryScrollCommand,
  commandKey: string,
  targetIndex: number,
): Extract<TranscriptPositioningEffect, { type: 'position' }> {
  if (command.kind === 'jump_to_message') {
    return { type: 'position', command, commandKey, targetIndex, align: 'start' };
  }
  return {
    type: 'position',
    command,
    commandKey,
    targetIndex,
    align: 'start',
    viewportStartOffset: command.viewportStartOffset,
  };
}

function targetMessageId(command: HistoryScrollCommand): string {
  return command.kind === 'jump_to_message' ? command.targetMessageId : command.messageId;
}

function rangeContains(range: TranscriptVisibleRange | null, index: number): boolean {
  return range !== null && range.startIndex <= index && index <= range.endIndex;
}
