import type { PaletteState, PaletteEvent, PaletteItem, PaletteSource, PaletteAction } from './types';
import { fuzzyMatch } from './fuzzyMatch';

export const initialState: PaletteState = { status: 'closed' };

/**
 * Pure state transition function for the command palette.
 * All state changes go through here — no side effects.
 */
export function transition(
  state: PaletteState,
  event: PaletteEvent,
  context?: { sources: PaletteSource[]; actions: PaletteAction[] },
): PaletteState {
  switch (event.type) {
    case 'OPEN': {
      if (state.status === 'open') return state;
      const results = context ? getSearchResults('', context.sources) : [];
      return {
        status: 'open',
        mode: 'search',
        query: '',
        rawInput: '',
        selectedIndex: 0,
        results,
      };
    }

    case 'CLOSE':
      return { status: 'closed' };

    case 'SET_QUERY': {
      if (state.status !== 'open') return state;
      const rawInput = event.rawInput;
      const isAction = rawInput.startsWith('>');
      const query = isAction ? rawInput.slice(1).trimStart() : rawInput;
      const mode = isAction ? 'action' : 'search';

      const results = context
        ? mode === 'action'
          ? getActionResults(query, context.actions)
          : getSearchResults(query, context.sources)
        : [];

      return {
        status: 'open',
        mode,
        query,
        rawInput,
        selectedIndex: 0,
        results,
      };
    }

    case 'SELECT_NEXT': {
      if (state.status !== 'open' || state.results.length === 0) return state;
      const next = state.selectedIndex + 1;
      return {
        ...state,
        selectedIndex: next >= state.results.length ? state.results.length - 1 : next,
      };
    }

    case 'SELECT_PREV': {
      if (state.status !== 'open' || state.results.length === 0) return state;
      const prev = state.selectedIndex - 1;
      return {
        ...state,
        selectedIndex: prev < 0 ? 0 : prev,
      };
    }

    case 'CONFIRM': {
      // Confirmation is handled by the component (side effect)
      // Transition to closed
      if (state.status !== 'open') return state;
      return { status: 'closed' };
    }
  }
}

// --- Result computation helpers ---

function getSearchResults(query: string, sources: PaletteSource[]): PaletteItem[] {
  const allResults: PaletteItem[] = [];
  for (const source of sources) {
    const items = source.search(query);
    allResults.push(...items);
  }
  return allResults;
}

function getActionResults(query: string, actions: PaletteAction[]): PaletteItem[] {
  const items: PaletteItem[] = actions.map(a => {
    const item: PaletteItem = {
      id: a.id,
      title: a.title,
      category: a.category || 'Actions',
      metadata: { actionId: a.id },
    };
    if (a.shortcut) item.subtitle = a.shortcut;
    if (a.icon) item.icon = a.icon;
    return item;
  });

  if (!query) return items;
  return fuzzyMatch(items, query, item => item.title);
}
