import { describe, expect, it } from 'vitest';
import type { ConversationState } from '../api';
import {
  decideRouteFocus,
  reduceRouteFocusState,
  type RouteFocusInputs,
  type RouteFocusState,
} from './conversationRouteFocus';

function idleState(): ConversationState {
  return { type: 'idle' };
}

function baseInputs(): RouteFocusInputs {
  return {
    isDesktop: true,
    routeKey: 'conversation:conv-1',
    routeSettled: true,
    browserSessionStateLoaded: true,
    archived: false,
    targetMessageId: undefined,
    activeFocusScope: null,
    viewerOwnsFocus: false,
    composerRenders: true,
    phase: idleState(),
  };
}

describe('decideRouteFocus', () => {
  const cases: ReadonlyArray<readonly [string, Partial<RouteFocusInputs>, RouteFocusState['decision']]> = [
    ['desktop live route focuses composer', {}, 'focus-composer'],
    ['mobile preserves owner', { isDesktop: false }, 'preserve-owner'],
    ['archived preserves owner', { archived: true }, 'preserve-owner'],
    ['message hash preserves owner', { targetMessageId: 'message-1' }, 'preserve-owner'],
    ['persistent focus scope preserves owner', { activeFocusScope: 'settings-dialog' }, 'preserve-owner'],
    ['route-content focus scope defers', { activeFocusScope: 'question-panel' }, 'pending'],
    ['viewer-owned preserves owner', { viewerOwnsFocus: true }, 'preserve-owner'],
    ['recovery preserves owner', { phase: { type: 'awaiting_recovery', message: 'recover', recovery_kind: 'resume', resume: { type: 'conversation_turn' } } }, 'preserve-owner'],
    ['continuation recovery preserves owner', { phase: { type: 'recoverable_continuation_failure', message: 'recover', error_kind: 'server_error', operation_id: 'op-1', attempt: 1 } }, 'preserve-owner'],
    ['question panel / no composer preserves owner', { composerRenders: false, phase: { type: 'awaiting_user_response', questions: [] } }, 'preserve-owner'],
    ['provisioning defers', { composerRenders: false, phase: { type: 'provisioning', prompt: null } }, 'pending'],
    ['awaiting llm defers even with a mounted textarea', { phase: { type: 'awaiting_llm' } }, 'pending'],
    ['awaiting continuation defers', { phase: { type: 'awaiting_continuation', attempt: 2 } }, 'pending'],
    ['browser restoration unsettled defers', { browserSessionStateLoaded: false }, 'pending'],
    ['route classification unsettled defers', { routeSettled: false }, 'pending'],
    ['missing route key is already consumed', { routeKey: null }, 'consumed'],
  ];

  it.each(cases)('%s', (_label, overrides, expected) => {
    expect(decideRouteFocus({ ...baseInputs(), ...overrides }).decision).toBe(expected);
  });
});

describe('reduceRouteFocusState', () => {
  it('consumes after focus and cannot retrigger from identical background refresh', () => {
    const route = decideRouteFocus(baseInputs());
    let state: RouteFocusState = { routeKey: null, decision: 'consumed' };

    state = reduceRouteFocusState(state, { type: 'route_observed', next: route });
    expect(state.decision).toBe('focus-composer');

    state = reduceRouteFocusState(state, { type: 'focus_applied' });
    expect(state.decision).toBe('consumed');

    state = reduceRouteFocusState(state, { type: 'route_observed', next: route });
    expect(state.decision).toBe('consumed');
  });

  it('treats canonical slug changes for the same conversation as the same route identity once consumed', () => {
    const focusByUuid = decideRouteFocus({ ...baseInputs(), routeKey: 'conversation:conv-1' });
    const focusBySlug = decideRouteFocus({ ...baseInputs(), routeKey: 'conversation:conv-1' });
    let state: RouteFocusState = { routeKey: null, decision: 'consumed' };

    state = reduceRouteFocusState(state, { type: 'route_observed', next: focusByUuid });
    state = reduceRouteFocusState(state, { type: 'focus_applied' });
    state = reduceRouteFocusState(state, { type: 'route_observed', next: focusBySlug });

    expect(state).toEqual({ routeKey: 'conversation:conv-1', decision: 'consumed' });
  });

  it('preserve-owner remains final for the same route after later phase changes', () => {
    const preserve = decideRouteFocus({ ...baseInputs(), archived: true });
    const laterEligible = decideRouteFocus({ ...baseInputs() });
    let state: RouteFocusState = { routeKey: null, decision: 'consumed' };

    state = reduceRouteFocusState(state, { type: 'route_observed', next: preserve });
    expect(state).toEqual({ routeKey: 'conversation:conv-1', decision: 'preserve-owner' });

    state = reduceRouteFocusState(state, { type: 'route_observed', next: laterEligible });
    expect(state).toEqual({ routeKey: 'conversation:conv-1', decision: 'preserve-owner' });
  });

  it('allows a pending route to resolve into focus for the same route key', () => {
    const pending = decideRouteFocus({ ...baseInputs(), browserSessionStateLoaded: false });
    const ready = decideRouteFocus(baseInputs());
    let state: RouteFocusState = { routeKey: null, decision: 'consumed' };

    state = reduceRouteFocusState(state, { type: 'route_observed', next: pending });
    expect(state).toEqual({ routeKey: 'conversation:conv-1', decision: 'pending' });

    state = reduceRouteFocusState(state, { type: 'route_observed', next: ready });
    expect(state).toEqual({ routeKey: 'conversation:conv-1', decision: 'focus-composer' });
  });

  it('preserves an intervening interaction instead of applying deferred focus later', () => {
    let state = decideRouteFocus({ ...baseInputs(), phase: { type: 'awaiting_llm' } });
    expect(state.decision).toBe('pending');

    state = reduceRouteFocusState(state, {
      type: 'interaction_claimed',
      routeKey: 'conversation:conv-1',
    });
    expect(state.decision).toBe('preserve-owner');

    state = reduceRouteFocusState(state, {
      type: 'route_observed',
      next: decideRouteFocus(baseInputs()),
    });
    expect(state.decision).toBe('preserve-owner');
  });

  it('carries an early interaction claim from unresolved route intent to authoritative identity', () => {
    let state: RouteFocusState = { routeKey: 'route:slug-a', decision: 'pending' };
    state = reduceRouteFocusState(state, {
      type: 'interaction_claimed',
      routeKey: 'route:slug-a',
    });
    state = reduceRouteFocusState(state, {
      type: 'route_observed',
      continuesRouteKey: 'route:slug-a',
      next: decideRouteFocus({ ...baseInputs(), routeKey: 'conversation:conv-1' }),
    });
    expect(state).toEqual({ routeKey: 'conversation:conv-1', decision: 'preserve-owner' });
  });

  it('allows a new route key to make a fresh decision', () => {
    let state: RouteFocusState = { routeKey: 'conversation:conv-1', decision: 'consumed' };
    state = reduceRouteFocusState(state, {
      type: 'route_observed',
      next: decideRouteFocus({ ...baseInputs(), routeKey: 'conversation:conv-2' }),
    });
    expect(state).toEqual({ routeKey: 'conversation:conv-2', decision: 'focus-composer' });
  });
});
