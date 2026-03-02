import type { PaletteState, PaletteEvent, PaletteItem, PaletteAction } from './types';
import { fuzzyMatch } from './fuzzyMatch';

export const initialState: PaletteState = { status: 'closed' };

/**
 * Pure state transition function for the command palette.
 * All state changes go through here — no side effects.
 *
 * Sources are async and handled outside the state machine (in the component
 * via useEffect). The state machine only needs actions for the synchronous
 * action-mode fuzzy match.
 */
export function transition(
  state: PaletteState,
  event: PaletteEvent,
  actions?: PaletteAction[],
): PaletteState {
  switch (event.type) {
    case 'OPEN': {
      if (state.status === 'open') return state;
      return {
        status: 'open',
        mode: 'search',
        query: '',
        rawInput: '',
        selectedIndex: 0,
        results: [],
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

      // Action mode: compute results synchronously from in-memory list.
      // Search mode: leave results stale — component useEffect fires async search
      // and dispatches SET_RESULTS when done.
      const results = isAction
        ? getActionResults(query, actions ?? [])
        : state.results;

      return {
        ...state,
        mode,
        query,
        rawInput,
        selectedIndex: 0,
        results,
      };
    }

    case 'SET_RESULTS': {
      if (state.status !== 'open') return state;
      return { ...state, results: event.results, selectedIndex: 0 };
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
      if (state.status !== 'open') return state;
      return { status: 'closed' };
    }
  }
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
