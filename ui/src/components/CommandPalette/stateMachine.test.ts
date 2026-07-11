import { describe, expect, it } from 'vitest';
import { initialState, transition } from './stateMachine';

function openPalette() {
  return transition(initialState, { type: 'OPEN' });
}

describe('command palette query parsing', () => {
  it('treats c followed by whitespace as conversation scope', () => {
    const scoped = transition(openPalette(), { type: 'SET_QUERY', rawInput: 'c ' });
    expect(scoped).toMatchObject({
      status: 'open',
      mode: 'search',
      scope: 'conversations',
      query: '',
      rawInput: 'c ',
      selectedIndex: 0,
    });

    const filtered = transition(scoped, { type: 'SET_QUERY', rawInput: 'c emo' });
    expect(filtered).toMatchObject({
      status: 'open',
      mode: 'search',
      scope: 'conversations',
      query: 'emo',
      rawInput: 'c emo',
      selectedIndex: 0,
    });
  });

  it('leaves words beginning with c in global search', () => {
    const state = transition(openPalette(), { type: 'SET_QUERY', rawInput: 'code' });
    expect(state).toMatchObject({
      status: 'open',
      mode: 'search',
      scope: 'global',
      query: 'code',
    });
  });

  it('keeps action mode parsing unchanged', () => {
    const state = transition(openPalette(), { type: 'SET_QUERY', rawInput: '> c emo' });
    expect(state).toMatchObject({
      status: 'open',
      mode: 'action',
      scope: 'global',
      query: 'c emo',
    });
  });
});
