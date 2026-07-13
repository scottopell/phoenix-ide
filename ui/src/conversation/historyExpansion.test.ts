import { describe, expect, it } from 'vitest';
import {
  initialHistoryExpansionState,
  reduceHistoryExpansion,
  type ActiveHistoryRequest,
  type HistoryView,
} from './historyExpansion';

const view = (conversationId: string, generation: number, transcriptGeneration = 1): HistoryView => ({
  conversationId,
  generation,
  transcriptGeneration,
});

const manualRequest = (currentView: HistoryView, token = 1): ActiveHistoryRequest => ({
  token,
  view: currentView,
  snapshotStartedAtEventSeq: 4,
  intent: { kind: 'manual_expansion', restore: { kind: 'reader_anchor', messageId: 'm50', viewportStartOffset: 12 } },
});

const deepLinkRequest = (currentView: HistoryView, token = 1): ActiveHistoryRequest => ({
  token,
  view: currentView,
  snapshotStartedAtEventSeq: 4,
  intent: { kind: 'deep_link', targetMessageId: 'm1' },
});

describe('history expansion reducer', () => {
  it('rejects an A generation 1 response after A generation 3 replaces it', () => {
    const a1 = view('a', 1);
    const a3 = view('a', 3);
    let state = reduceHistoryExpansion(initialHistoryExpansionState(a1, true), {
      type: 'request_started', request: manualRequest(a1),
    });
    state = reduceHistoryExpansion(state, { type: 'view_changed', view: a3, hasEarlierHistory: true });

    expect(reduceHistoryExpansion(state, {
      type: 'history_loaded', requestToken: 1, view: a1, targetPresent: true, commandToken: 101,
    })).toBe(state);
  });

  it('manual reader expansion creates one continuity command', () => {
    let state = reduceHistoryExpansion(initialHistoryExpansionState(view('a', 1), true), {
      type: 'request_started', request: manualRequest(view('a', 1)),
    });
    state = reduceHistoryExpansion(state, {
      type: 'history_loaded', requestToken: 1, view: view('a', 1), targetPresent: true, commandToken: 101,
    });

    expect(state.pendingCommand).toMatchObject({
      kind: 'restore_after_prefix_expansion', messageId: 'm50', viewportStartOffset: 12, token: 101, requestToken: 1,
    });
    const duplicate = reduceHistoryExpansion(state, {
      type: 'history_loaded', requestToken: 1, view: view('a', 1), targetPresent: true, commandToken: 101,
    });
    expect(duplicate).toBe(state);
  });

  it('manual expansion while following creates no positioning command', () => {
    const currentView = view('a', 1);
    const request: ActiveHistoryRequest = {
      ...manualRequest(currentView),
      intent: { kind: 'manual_expansion', restore: { kind: 'following_tail' } },
    };
    let state = reduceHistoryExpansion(initialHistoryExpansionState(currentView, true), {
      type: 'request_started', request,
    });
    state = reduceHistoryExpansion(state, {
      type: 'history_loaded', requestToken: 1, view: currentView, targetPresent: true, commandToken: 101,
    });
    expect(state.pendingCommand).toBeNull();
    expect(state.coverage).toBe('complete');
  });

  it('deep link gets one jump command or a terminal not-found outcome', () => {
    const currentView = view('a', 1);
    let found = reduceHistoryExpansion(initialHistoryExpansionState(currentView, true), {
      type: 'request_started', request: deepLinkRequest(currentView),
    });
    found = reduceHistoryExpansion(found, {
      type: 'history_loaded', requestToken: 1, view: currentView, targetPresent: true, commandToken: 101,
    });
    expect(found.pendingCommand).toMatchObject({ kind: 'jump_to_message', targetMessageId: 'm1' });

    let missing = reduceHistoryExpansion(initialHistoryExpansionState(currentView, true), {
      type: 'request_started', request: deepLinkRequest(currentView),
    });
    missing = reduceHistoryExpansion(missing, {
      type: 'history_loaded', requestToken: 1, view: currentView, targetPresent: false, commandToken: 101,
    });
    expect(missing.failure).toEqual({ kind: 'target_not_found', targetMessageId: 'm1' });
    expect(missing.activeRequest).toBeNull();
  });

  it('matching acknowledgement consumes once and stale acknowledgement is ignored', () => {
    const currentView = view('a', 1);
    let state = reduceHistoryExpansion(initialHistoryExpansionState(currentView, true), {
      type: 'request_started', request: manualRequest(currentView),
    });
    state = reduceHistoryExpansion(state, {
      type: 'history_loaded', requestToken: 1, view: currentView, targetPresent: true, commandToken: 101,
    });
    const wrong = reduceHistoryExpansion(state, {
      type: 'command_acknowledged', commandToken: 2, view: currentView, result: 'applied',
    });
    expect(wrong).toBe(state);
    state = reduceHistoryExpansion(state, {
      type: 'command_acknowledged', commandToken: 101, view: currentView, result: 'applied',
    });
    expect(state.pendingCommand).toBeNull();
    expect(reduceHistoryExpansion(state, {
      type: 'command_acknowledged', commandToken: 101, view: currentView, result: 'applied',
    })).toBe(state);
  });

  it('rejects a completion from the wrong transcript generation', () => {
    const currentView = view('a', 1, 2);
    const state = reduceHistoryExpansion(initialHistoryExpansionState(currentView, true), {
      type: 'request_started', request: manualRequest(currentView),
    });
    expect(reduceHistoryExpansion(state, {
      type: 'history_loaded', requestToken: 1, view: view('a', 1, 1), targetPresent: true, commandToken: 101,
    })).toBe(state);
  });

  it('rejects a failure from the wrong transcript generation and preserves the active request', () => {
    const currentView = view('a', 1, 2);
    const state = reduceHistoryExpansion(initialHistoryExpansionState(currentView, true), {
      type: 'request_started', request: manualRequest(currentView),
    });

    expect(reduceHistoryExpansion(state, {
      type: 'history_failed',
      requestToken: 1,
      view: currentView,
      transcriptGeneration: 1,
      message: 'offline',
    })).toBe(state);
  });

  it('manual retry with a retained deep-link failure intent clears and replaces the failure', () => {
    const currentView = view('a', 1);
    const first = deepLinkRequest(currentView, 1);
    let state = reduceHistoryExpansion(initialHistoryExpansionState(currentView, true), {
      type: 'request_started', request: first,
    });
    state = reduceHistoryExpansion(state, {
      type: 'history_failed', requestToken: 1, view: currentView, transcriptGeneration: currentView.transcriptGeneration, message: 'offline',
    });
    const beforeRetry = state;
    const retry = deepLinkRequest(currentView, 2);
    state = reduceHistoryExpansion(state, { type: 'request_started', request: retry });
    expect(state).not.toBe(beforeRetry);
    expect(state.activeRequest).toEqual(retry);
    expect(state.failure).toBeNull();
  });

  it('manual retry with a retained manual-expansion failure intent clears and replaces the failure', () => {
    const currentView = view('a', 1);
    const first = manualRequest(currentView, 1);
    let state = reduceHistoryExpansion(initialHistoryExpansionState(currentView, true), {
      type: 'request_started', request: first,
    });
    state = reduceHistoryExpansion(state, {
      type: 'history_failed', requestToken: 1, view: currentView, transcriptGeneration: currentView.transcriptGeneration, message: 'offline',
    });
    const retry = manualRequest(currentView, 2);

    state = reduceHistoryExpansion(state, { type: 'request_started', request: retry });

    expect(state.activeRequest).toEqual(retry);
    expect(state.failure).toBeNull();
  });

  it('creates a navigation command for a target already present in tail coverage', () => {
    const currentView = view('a', 1);
    const state = initialHistoryExpansionState(currentView, true);
    const next = reduceHistoryExpansion(state, {
      type: 'loaded_target_requested',
      targetMessageId: 'm42',
      commandToken: 7,
    });

    expect(next.coverage).toBe('tail');
    expect(next.pendingCommand?.kind).toBe('jump_to_message');
  });

  it('clears a missing-target failure when the deep-link target changes', () => {
    const currentView = view('a', 1);
    const state = {
      ...initialHistoryExpansionState(currentView, false),
      failure: { kind: 'target_not_found' as const, targetMessageId: 'missing' },
    };

    const next = reduceHistoryExpansion(state, {
      type: 'target_changed',
      targetMessageId: 'loaded',
    });

    expect(next.failure).toBeNull();
  });

  it('retains a missing-target failure while the same target remains active', () => {
    const currentView = view('a', 1);
    const state = {
      ...initialHistoryExpansionState(currentView, false),
      failure: { kind: 'target_not_found' as const, targetMessageId: 'missing' },
    };

    expect(reduceHistoryExpansion(state, {
      type: 'target_changed',
      targetMessageId: 'missing',
    })).toBe(state);
  });

  it('does not reissue a known failed deep-link target', () => {
    const currentView = view('a', 1);
    const state = {
      ...initialHistoryExpansionState(currentView, false),
      failure: { kind: 'target_not_found' as const, targetMessageId: 'm42' },
    };
    const next = reduceHistoryExpansion(state, {
      type: 'loaded_target_requested',
      targetMessageId: 'm42',
      commandToken: 7,
    });

    expect(next).toBe(state);
  });

  it('invalidates stale deep-link work when the target changes', () => {
    const currentView = view('a', 1);
    let state = reduceHistoryExpansion(initialHistoryExpansionState(currentView, true), {
      type: 'request_started', request: deepLinkRequest(currentView),
    });
    state = reduceHistoryExpansion(state, { type: 'target_changed', targetMessageId: 'm2' });

    expect(state.activeRequest).toBeNull();
    expect(reduceHistoryExpansion(state, {
      type: 'history_loaded', requestToken: 1, view: currentView, targetPresent: true, commandToken: 101,
    })).toBe(state);
  });

  it('clears a jump command for the previous target', () => {
    const currentView = view('a', 1);
    let state = reduceHistoryExpansion(initialHistoryExpansionState(currentView, true), {
      type: 'loaded_target_requested', targetMessageId: 'm1', commandToken: 7,
    });

    state = reduceHistoryExpansion(state, { type: 'target_changed', targetMessageId: 'm2' });
    expect(state.pendingCommand).toBeNull();
  });

  it('keeps manual expansion active when the deep-link target changes', () => {
    const currentView = view('a', 1);
    const state = reduceHistoryExpansion(initialHistoryExpansionState(currentView, true), {
      type: 'request_started', request: manualRequest(currentView),
    });

    expect(reduceHistoryExpansion(state, {
      type: 'target_changed', targetMessageId: 'm2',
    })).toBe(state);
  });

  it('clears a request failure when the deep-link target changes', () => {
    const currentView = view('a', 1);
    const state = {
      ...initialHistoryExpansionState(currentView, true),
      failure: {
        kind: 'request_failed' as const,
        message: 'offline',
        intent: { kind: 'deep_link' as const, targetMessageId: 'm1' },
        transcriptGeneration: currentView.transcriptGeneration,
      },
    };

    expect(reduceHistoryExpansion(state, {
      type: 'target_changed', targetMessageId: 'm2',
    }).failure).toBeNull();
  });

  it('failure leaves tail coverage usable and cannot retry itself', () => {
    const currentView = view('a', 1);
    const request = deepLinkRequest(currentView);
    let state = reduceHistoryExpansion(initialHistoryExpansionState(currentView, true), {
      type: 'request_started', request,
    });
    state = reduceHistoryExpansion(state, {
      type: 'history_failed', requestToken: 1, view: currentView, transcriptGeneration: currentView.transcriptGeneration, message: 'offline',
    });
    expect(state).toMatchObject({ coverage: 'tail', activeRequest: null, pendingCommand: null });
    expect(state.failure).toEqual({
      kind: 'request_failed',
      message: 'offline',
      intent: request.intent,
      transcriptGeneration: currentView.transcriptGeneration,
    });
  });
});
