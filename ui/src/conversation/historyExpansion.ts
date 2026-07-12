export type HistoryView = {
  conversationId: string;
  generation: number;
  transcriptGeneration: number;
};

export type RestoreBasis =
  | { kind: 'reader_anchor'; messageId: string; viewportStartOffset: number }
  | { kind: 'following_tail' };

export type HistoryIntent =
  | { kind: 'manual_expansion'; restore: RestoreBasis }
  | { kind: 'deep_link'; targetMessageId: string };

export type ActiveHistoryRequest = {
  token: number;
  view: HistoryView;
  snapshotStartedAtEventSeq: number;
  intent: HistoryIntent;
};

export type HistoryCommandToken = number;

export type HistoryScrollCommand =
  | {
      kind: 'restore_after_prefix_expansion';
      token: HistoryCommandToken;
      requestToken: number;
      view: HistoryView;
      messageId: string;
      viewportStartOffset: number;
    }
  | {
      kind: 'jump_to_message';
      token: HistoryCommandToken;
      requestToken: number;
      view: HistoryView;
      targetMessageId: string;
    };

export type HistoryFailure =
  | { kind: 'request_failed'; message: string; intent: HistoryIntent }
  | { kind: 'target_not_found'; targetMessageId: string }
  | { kind: 'anchor_not_found'; anchorMessageId: string };

export type HistoryExpansionState = {
  view: HistoryView;
  coverage: 'tail' | 'complete';
  activeRequest: ActiveHistoryRequest | null;
  pendingCommand: HistoryScrollCommand | null;
  failure: HistoryFailure | null;
};

export type HistoryExpansionEvent =
  | { type: 'view_changed'; view: HistoryView; hasEarlierHistory: boolean }
  | { type: 'request_started'; request: ActiveHistoryRequest }
  | { type: 'target_changed'; targetMessageId: string | null }
  | { type: 'loaded_target_requested'; targetMessageId: string; commandToken: HistoryCommandToken }
  | {
      type: 'history_loaded';
      requestToken: number;
      view: HistoryView;
      targetPresent: boolean;
      commandToken: HistoryCommandToken;
    }
  | { type: 'history_failed'; requestToken: number; view: HistoryView; message: string }
  | {
      type: 'command_acknowledged';
      commandToken: number;
      view: HistoryView;
      result: 'applied' | 'target_missing' | 'superseded';
    };

export function initialHistoryExpansionState(
  view: HistoryView,
  hasEarlierHistory: boolean,
): HistoryExpansionState {
  return {
    view,
    coverage: hasEarlierHistory ? 'tail' : 'complete',
    activeRequest: null,
    pendingCommand: null,
    failure: null,
  };
}

function sameView(left: HistoryView, right: HistoryView): boolean {
  return left.conversationId === right.conversationId
    && left.generation === right.generation
    && left.transcriptGeneration === right.transcriptGeneration;
}

export function reduceHistoryExpansion(
  state: HistoryExpansionState,
  event: HistoryExpansionEvent,
): HistoryExpansionState {
  switch (event.type) {
    case 'view_changed':
      return initialHistoryExpansionState(event.view, event.hasEarlierHistory);

    case 'target_changed': {
      const staleRequest = state.activeRequest?.intent.kind === 'deep_link'
        && state.activeRequest.intent.targetMessageId !== event.targetMessageId;
      const staleCommand = state.pendingCommand?.kind === 'jump_to_message'
        && state.pendingCommand.targetMessageId !== event.targetMessageId;
      const staleFailure = state.failure?.kind === 'request_failed'
        || (state.failure?.kind === 'target_not_found'
          && state.failure.targetMessageId !== event.targetMessageId);
      if (!staleRequest && !staleCommand && !staleFailure) return state;
      return {
        ...state,
        activeRequest: staleRequest ? null : state.activeRequest,
        pendingCommand: staleCommand ? null : state.pendingCommand,
        failure: staleFailure ? null : state.failure,
      };
    }

    case 'loaded_target_requested':
      if (state.pendingCommand || state.failure) return state;
      return {
        ...state,
        pendingCommand: {
          kind: 'jump_to_message',
          token: event.commandToken,
          requestToken: 0,
          view: state.view,
          targetMessageId: event.targetMessageId,
        },
        failure: null,
      };

    case 'request_started':
      if (state.coverage !== 'tail' || state.activeRequest || !sameView(state.view, event.request.view)) {
        return state;
      }
      return { ...state, activeRequest: event.request, pendingCommand: null, failure: null };

    case 'history_loaded': {
      const request = state.activeRequest;
      if (!request || request.token !== event.requestToken || !sameView(request.view, event.view)) return state;
      if (request.intent.kind === 'deep_link' && !event.targetPresent) {
        return {
          ...state,
          coverage: 'complete',
          activeRequest: null,
          pendingCommand: null,
          failure: { kind: 'target_not_found', targetMessageId: request.intent.targetMessageId },
        };
      }
      const pendingCommand: HistoryScrollCommand | null = request.intent.kind === 'deep_link'
        ? {
            kind: 'jump_to_message',
            token: event.commandToken,
            requestToken: request.token,
            view: request.view,
            targetMessageId: request.intent.targetMessageId,
          }
        : request.intent.restore.kind === 'reader_anchor'
          ? {
              kind: 'restore_after_prefix_expansion',
              token: event.commandToken,
              requestToken: request.token,
              view: request.view,
              messageId: request.intent.restore.messageId,
              viewportStartOffset: request.intent.restore.viewportStartOffset,
            }
          : null;
      return {
        ...state,
        coverage: 'complete',
        activeRequest: null,
        pendingCommand,
        failure: null,
      };
    }

    case 'history_failed': {
      const request = state.activeRequest;
      if (!request || request.token !== event.requestToken || !sameView(request.view, event.view)) return state;
      return {
        ...state,
        activeRequest: null,
        pendingCommand: null,
        failure: { kind: 'request_failed', message: event.message, intent: request.intent },
      };
    }

    case 'command_acknowledged': {
      const command = state.pendingCommand;
      if (!command || command.token !== event.commandToken || !sameView(command.view, event.view)) return state;
      if (event.result === 'target_missing') {
        const failure = command.kind === 'jump_to_message'
          ? { kind: 'target_not_found' as const, targetMessageId: command.targetMessageId }
          : { kind: 'anchor_not_found' as const, anchorMessageId: command.messageId };
        return { ...state, pendingCommand: null, failure };
      }
      return { ...state, pendingCommand: null };
    }
  }
}
