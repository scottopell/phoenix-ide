import { useCallback, useEffect, useMemo, useReducer } from 'react';
import { findLiteralMatches, type ViewerFindResult } from './literalMatch';
import { initialViewerFindState, viewerFindReducer } from './viewerFindReducer';

export interface ViewerFindNavigateContext {
  query: string;
  result: ViewerFindResult;
  activeIndex: number;
}

export interface UseViewerFindOptions {
  text: string;
  onNavigate?: (context: ViewerFindNavigateContext) => void;
}

export function useViewerFind({ text, onNavigate }: UseViewerFindOptions) {
  const [state, dispatch] = useReducer(viewerFindReducer, initialViewerFindState);

  const result = useMemo(() => findLiteralMatches(text, state.query), [text, state.query]);
  const matchCount = result.matches.length;
  const activeIndex = matchCount === 0
    ? -1
    : state.activeIndex < 0
      ? 0
      : Math.min(state.activeIndex, matchCount - 1);
  const activeMatch = activeIndex >= 0 ? result.matches[activeIndex] ?? null : null;

  useEffect(() => {
    if (!state.isOpen || activeIndex < 0 || !onNavigate) return;
    onNavigate({ query: state.query, result, activeIndex });
  }, [activeIndex, onNavigate, result, state.isOpen, state.query]);

  const open = useCallback(() => {
    dispatch({ type: 'open' });
  }, []);

  const close = useCallback(() => {
    dispatch({ type: 'close' });
  }, []);

  const toggle = useCallback(() => {
    dispatch({ type: 'toggle' });
  }, []);

  const setQuery = useCallback((query: string) => {
    dispatch({ type: 'set-query', query });
  }, []);

  const nextMatch = useCallback(() => {
    dispatch({ type: 'next-match', matchCount });
  }, [matchCount]);

  const previousMatch = useCallback(() => {
    dispatch({ type: 'previous-match', matchCount });
  }, [matchCount]);

  const setActiveIndex = useCallback((index: number) => {
    dispatch({ type: 'set-active-index', index });
  }, []);

  return {
    isOpen: state.isOpen,
    query: state.query,
    result,
    matches: result.matches,
    matchCount,
    activeIndex,
    activeMatch,
    focusVersion: state.focusVersion,
    open,
    close,
    toggle,
    setQuery,
    nextMatch,
    previousMatch,
    setActiveIndex,
  };
}

export type UseViewerFindReturn = ReturnType<typeof useViewerFind>;
