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

  it('clears stale search results before an async scoped search completes', () => {
    const open = openPalette();
    if (open.status !== 'open') throw new Error('palette did not open');
    const withGlobalResults = transition(open, {
      type: 'SET_RESULTS',
      results: [{
        id: 'src/main.rs',
        title: 'main.rs',
        category: 'Files',
        sourceId: 'files',
      }],
    });

    const scoped = transition(withGlobalResults, { type: 'SET_QUERY', rawInput: 'c ' });
    expect(scoped).toMatchObject({
      status: 'open',
      scope: 'conversations',
      selectedIndex: 0,
      results: [],
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
