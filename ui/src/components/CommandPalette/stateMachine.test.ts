import { describe, expect, it } from 'vitest';
import { initialState, transition } from './stateMachine';

function openPalette() {
  return transition(initialState, { type: 'OPEN' });
}

describe('command palette query parsing', () => {
  it('treats c followed by whitespace as conversation content scope', () => {
    const scoped = transition(openPalette(), { type: 'SET_QUERY', rawInput: 'c ' });
    expect(scoped).toMatchObject({
      status: 'open',
      mode: 'search',
      scope: 'conversation-content',
      query: '',
      rawInput: 'c ',
      selectedIndex: 0,
    });

    const filtered = transition(scoped, { type: 'SET_QUERY', rawInput: 'c emo' });
    expect(filtered).toMatchObject({
      status: 'open',
      mode: 'search',
      scope: 'conversation-content',
      query: 'emo',
      rawInput: 'c emo',
      selectedIndex: 0,
    });
  });

  it('prefers cs over c when both prefixes could match', () => {
    const scoped = transition(openPalette(), { type: 'SET_QUERY', rawInput: 'cs emo' });
    expect(scoped).toMatchObject({
      status: 'open',
      mode: 'search',
      scope: 'conversation-slugs',
      query: 'emo',
      rawInput: 'cs emo',
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
      scope: 'conversation-content',
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

describe('command palette async search status', () => {
  it('tracks debouncing, loading, warming, error, and ready states', () => {
    const open = transition(openPalette(), { type: 'SET_QUERY', rawInput: 'c bug' });
    expect(open).toMatchObject({
      status: 'open',
      searchStatus: { kind: 'idle' },
    });

    const debouncing = transition(open, { type: 'SEARCH_DEBOUNCING' });
    expect(debouncing).toMatchObject({ searchStatus: { kind: 'debouncing' } });

    const loading = transition(debouncing, { type: 'SEARCH_LOADING' });
    expect(loading).toMatchObject({ searchStatus: { kind: 'loading' } });

    const warming = transition(loading, { type: 'SEARCH_WARMING', message: 'Index warming' });
    expect(warming).toMatchObject({ searchStatus: { kind: 'warming', message: 'Index warming' } });

    const errored = transition(loading, { type: 'SEARCH_ERROR', message: 'Search failed' });
    expect(errored).toMatchObject({ searchStatus: { kind: 'error', message: 'Search failed' } });

    const ready = transition(loading, { type: 'SET_RESULTS', results: [] });
    expect(ready).toMatchObject({ searchStatus: { kind: 'ready' } });
  });
});
