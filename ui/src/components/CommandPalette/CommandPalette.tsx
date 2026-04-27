import { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { useNavigate, useLocation } from 'react-router-dom';
import type { Conversation } from '../../api';
import { api } from '../../api';
import type { PaletteState, PaletteSource, PaletteAction } from './types';
import { transition, initialState } from './stateMachine';
import { CommandPaletteInput } from './CommandPaletteInput';
import { CommandPaletteResults } from './CommandPaletteResults';
import { createConversationSource } from './sources/ConversationSource';
import { createFileSource } from './sources/FileSource';
import { createBuiltInActions } from './actions/builtInActions';
import { useFileExplorer } from '../../hooks/useFileExplorer';
import { useFocusScope } from '../../hooks/useFocusScope';

const SEARCH_DEBOUNCE_MS = 120;

interface CommandPaletteProps {
  conversations: Conversation[];
}

export function CommandPalette({ conversations }: CommandPaletteProps) {
  const [state, setState] = useState<PaletteState>(initialState);
  const [isDesktop, setIsDesktop] = useState(() => window.matchMedia('(min-width: 1025px)').matches);
  const navigate = useNavigate();
  const location = useLocation();
  const overlayRef = useRef<HTMLDivElement>(null);
  const [hoverIndex, setHoverIndex] = useState<number | null>(null);
  const searchAbortRef = useRef<AbortController | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const { openFile } = useFileExplorer();

  // Desktop detection
  useEffect(() => {
    const mq = window.matchMedia('(min-width: 1025px)');
    const handler = (e: MediaQueryListEvent) => setIsDesktop(e.matches);
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  }, []);

  // Extract current slug and active conversation
  const slugMatch = location.pathname.match(/^\/c\/(.+)$/);
  const currentSlug = slugMatch?.[1] ?? null;

  // Stable scalars for the active conversation — only update when id/cwd actually change,
  // not on every 5s poll that produces a new array reference from DesktopLayout.
  const activeConversationRaw = conversations.find(c => c.slug === currentSlug) ?? null;
  const activeConvId = activeConversationRaw?.id ?? null;
  const activeConvCwd = activeConversationRaw?.cwd ?? null;

  // Stable conversation ids string — only changes when the *set* of conversations changes.
  const conversationIdsKey = useMemo(
    () => conversations.map(c => c.id).join(','),
    [conversations],
  );

  // ConversationSource — recomputed only when the conversation set changes (by id key).
  // conversations ref changes every 5s (DesktopLayout poll) but conversationIdsKey is
  // stable across same-content polls; use key as the real dep, capture conversations.
  const conversationSource = useMemo(
    () => createConversationSource(conversations, (slug) => navigate(`/c/${slug}`)),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [conversationIdsKey, navigate],
  );

  // FileSource — recomputed only when conversation id or cwd actually changes.
  const fileSource = useMemo(
    () =>
      activeConvId && activeConvCwd
        ? createFileSource(activeConvId, activeConvCwd, (path, rootDir) => openFile(path, rootDir))
        : null,
    [activeConvId, activeConvCwd, openFile],
  );

  // Stable sources array — changes only when conversationSource or fileSource identity changes.
  const sources: PaletteSource[] = useMemo(
    () => (fileSource ? [conversationSource, fileSource] : [conversationSource]),
    [conversationSource, fileSource],
  );

  // Keep a ref so the search effect always sees the latest sources without
  // needing sources in its dep array (which would re-fire the effect on every
  // sources identity change, even when only the conversation list content shifted).
  const sourcesRef = useRef<PaletteSource[]>(sources);
  useEffect(() => {
    sourcesRef.current = sources;
  });

  // Stable boolean for downstream consumers — true when inside a conversation route.
  const hasActiveConversation = activeConvId !== null;

  const actions: PaletteAction[] = useMemo(
    () =>
      createBuiltInActions({
        navigate,
        currentSlug,
        archiveCurrent: currentSlug
          ? () => {
              const conv = conversations.find(c => c.slug === currentSlug);
              if (conv) {
                api.archiveConversation(conv.id).then(() => navigate('/'));
              }
            }
          : undefined,
      }),
    [navigate, currentSlug, conversations],
  );

  // Dispatch helper — state machine only needs actions now (sources are async)
  const dispatch = useCallback(
    (event: Parameters<typeof transition>[1]) => {
      setState(prev => transition(prev, event, actions));
      setHoverIndex(null);
    },
    [actions],
  );

  // Focus scope: register when palette is open, unregister when closed
  const { pushScope, popScope } = useFocusScope();
  useEffect(() => {
    if (state.status === 'open') {
      pushScope('command-palette');
      return () => popScope('command-palette');
    }
    return undefined;
  }, [state.status, pushScope, popScope]);

  // Async search effect — fires on query/mode change, debounced, abortable.
  // Depends on derived primitives, NOT on state object, to avoid re-firing
  // when SET_RESULTS updates state.results.
  const isOpen = state.status === 'open';
  const searchMode = state.status === 'open' ? state.mode : null;
  const searchQuery = state.status === 'open' ? state.query : null;

  useEffect(() => {
    if (!isOpen || searchMode !== 'search') return;
    const query = searchQuery ?? '';

    // Cancel previous debounce + in-flight request
    if (debounceRef.current) clearTimeout(debounceRef.current);
    searchAbortRef.current?.abort();

    debounceRef.current = setTimeout(async () => {
      const controller = new AbortController();
      searchAbortRef.current = controller;

      // Read sources via ref — latest value without making sources a dep.
      // This prevents the 5s conversation-poll from re-aborting in-flight requests.
      const allResults = await Promise.all(
        sourcesRef.current.map(s => s.search(query, controller.signal))
      );

      if (!controller.signal.aborted) {
        setState(prev => transition(prev, { type: 'SET_RESULTS', results: allResults.flat() }, actions));
      }
    }, SEARCH_DEBOUNCE_MS);

    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, [isOpen, searchMode, searchQuery, actions]);

  // Global Cmd/Ctrl+P shortcut (REQ-CP-001)
  useEffect(() => {
    if (!isDesktop) return;

    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'p') {
        e.preventDefault();
        e.stopPropagation();
        setState(prev => {
          if (prev.status === 'open') {
            return transition(prev, { type: 'CLOSE' }, actions);
          }
          return transition(prev, { type: 'OPEN' }, actions);
        });
      }
    };

    window.addEventListener('keydown', handler, true);
    return () => window.removeEventListener('keydown', handler, true);
  }, [isDesktop, actions]);

  // Close on route change
  useEffect(() => {
    setState(prev => (prev.status === 'open' ? { status: 'closed' } : prev));
  }, [location.pathname]);

  // Keyboard navigation within palette (REQ-CP-005)
  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          dispatch({ type: 'SELECT_NEXT' });
          break;
        case 'ArrowUp':
          e.preventDefault();
          dispatch({ type: 'SELECT_PREV' });
          break;
        case 'Enter':
          e.preventDefault();
          if (state.status === 'open' && state.results.length > 0) {
            const idx = hoverIndex ?? state.selectedIndex;
            const selected = state.results[idx];
            if (selected) {
              // Execute the selection side effect
              if (state.mode === 'search') {
                const source = sources.find(s => s.category === selected.category);
                source?.onSelect(selected);
              } else {
                const action = actions.find(a => a.id === selected.id);
                action?.handler();
              }
            }
            dispatch({ type: 'CONFIRM' });
          }
          break;
        case 'Escape':
          e.preventDefault();
          e.stopPropagation();
          dispatch({ type: 'CLOSE' });
          break;
        case 'n':
          if (e.ctrlKey) {
            e.preventDefault();
            dispatch({ type: 'SELECT_NEXT' });
          }
          break;
        case 'p':
          if (e.ctrlKey) {
            e.preventDefault();
            dispatch({ type: 'SELECT_PREV' });
          }
          break;
      }
    },
    [state, dispatch, sources, actions, hoverIndex],
  );

  // Handle hover over results
  const handleHover = useCallback(
    (index: number) => setHoverIndex(index),
    [],
  );

  // Handle click on a result
  const handleClick = useCallback(
    (index: number) => {
      if (state.status !== 'open') return;
      const selected = state.results[index];
      if (selected) {
        if (state.mode === 'search') {
          const source = sources.find(s => s.category === selected.category);
          source?.onSelect(selected);
        } else {
          const action = actions.find(a => a.id === selected.id);
          action?.handler();
        }
      }
      dispatch({ type: 'CONFIRM' });
    },
    [state, dispatch, sources, actions],
  );

  // Don't render on mobile or when closed (REQ-CP-008)
  if (!isDesktop || state.status === 'closed') return null;

  const effectiveIndex = hoverIndex ?? state.selectedIndex;

  return (
    <div
      className="cp-overlay"
      ref={overlayRef}
      onClick={(e) => {
        if (e.target === overlayRef.current) {
          dispatch({ type: 'CLOSE' });
        }
      }}
    >
      <div className="cp-container">
        <CommandPaletteInput
          value={state.rawInput}
          mode={state.mode}
          hasActiveConversation={hasActiveConversation}
          onChange={(value) => dispatch({ type: 'SET_QUERY', rawInput: value })}
          onKeyDown={handleKeyDown}
        />
        <CommandPaletteResults
          results={state.results}
          selectedIndex={effectiveIndex}
          mode={state.mode}
          onHover={handleHover}
          onClick={handleClick}
        />
      </div>
    </div>
  );
}
