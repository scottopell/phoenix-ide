import { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { useNavigate, useLocation } from 'react-router-dom';
import type { Conversation, ProductConversationListRow } from '../../api';
import { api } from '../../api';
import { ConversationSearchWarmingError } from '../../api';
import './CommandPalette.css';
import type { PaletteState, PaletteSource, PaletteAction } from './types';
import { transition, initialState } from './stateMachine';
import { CommandPaletteInput } from './CommandPaletteInput';
import { CommandPaletteResults } from './CommandPaletteResults';
import { createConversationSource } from './sources/ConversationSource';
import { createFileSource } from './sources/FileSource';
import { createCodeSource } from './sources/CodeSource';
import { createConversationContentSource } from './sources/ConversationContentSource';
import { createBuiltInActions } from './actions/builtInActions';
import { useFileExplorer } from '../../hooks/useFileExplorer';
import { notifyArchiveCloseConflict } from '../../notifications';
import { computeChainRoots } from '../../utils/chains';
import { useFocusScope } from '../../hooks/useFocusScope';
import { useIsDesktop } from '../../hooks/useMediaQuery';
import { activeConversationFileRoot } from './fileRoot';

const SEARCH_DEBOUNCE_MS = 120;

interface CommandPaletteProps {
  conversations: readonly Conversation[];
  productConversations?: readonly ProductConversationListRow[];
  activeConversation: Conversation | null;
}

export function CommandPalette({ conversations, productConversations = [], activeConversation }: CommandPaletteProps) {
  const [state, setState] = useState<PaletteState>(initialState);
  const isDesktop = useIsDesktop();
  const navigate = useNavigate();
  const location = useLocation();
  const overlayRef = useRef<HTMLDivElement>(null);
  const [hoverIndex, setHoverIndex] = useState<number | null>(null);
  const searchAbortRef = useRef<AbortController | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const { openFile } = useFileExplorer();

  // Extract current slug and active conversation
  const slugMatch = location.pathname.match(/^\/c\/(.+)$/);
  const productMatch = location.pathname.match(/^\/product-conversations\/([^/]+)$/);
  const currentSlug = slugMatch?.[1] ?? null;
  const activeProduct = productMatch
    ? productConversations.find(row => row.product_conversation_id === productMatch[1])
    : undefined;

  const activeConvId = activeConversation?.id ?? null;
  const activeFileRoot = activeConversationFileRoot(activeConversation);

  // Stable conversation ids string — only changes when the *set* of conversations changes.
  const conversationIdsKey = useMemo(
    () => conversations.map(c => c.id).join(','),
    [conversations],
  );
  const productConversationIdsKey = useMemo(
    () => productConversations.map(row => `${row.product_conversation_id}:${row.updated_at}:${row.presentation.display_name}`).join(','),
    [productConversations],
  );

  // ConversationSource — recomputed only when the conversation set changes (by id key).
  // conversations ref changes every 5s (DesktopLayout poll) but conversationIdsKey is
  // stable across same-content polls; use key as the real dep, capture conversations.
  const conversationSource = useMemo(
    () => createConversationSource(
      conversations,
      (slug) => navigate(`/c/${slug}`),
      productConversations,
      (route) => navigate(route),
    ),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [conversationIdsKey, productConversationIdsKey, navigate],
  );
  const conversationContentSource = useMemo(
    () => createConversationContentSource((slug) => navigate(`/c/${slug}`)),
    [navigate],
  );

  // FileSource and CodeSource — recomputed only when conversation id or file root actually changes.
  const fileSource = useMemo(
    () =>
      activeConvId && activeFileRoot
        ? createFileSource(activeConvId, activeFileRoot, (path, rootDir, options) => openFile(path, rootDir, options))
        : null,
    [activeConvId, activeFileRoot, openFile],
  );
  const codeSource = useMemo(
    () =>
      activeConvId && activeFileRoot
        ? createCodeSource(activeConvId, activeFileRoot, (path, rootDir, options) => openFile(path, rootDir, options))
        : null,
    [activeConvId, activeFileRoot, openFile],
  );

  // Stable sources array — changes only when source identities change.
  const sources: PaletteSource[] = useMemo(
    () => {
      const scopedSources = fileSource && codeSource ? [fileSource, codeSource] : [];
      return [...scopedSources, conversationSource, conversationContentSource];
    },
    [conversationSource, conversationContentSource, fileSource, codeSource],
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
        currentSlug: currentSlug ?? activeProduct?.canonical_root.slug ?? null,
        archiveCurrent: currentSlug || activeProduct?.ordinary_lifecycle === 'open'
          ? (() => {
              const activeRoute = activeConvId ?? activeProduct?.latest_transcript_row_id ?? currentSlug;
              const conv = conversations.find(c => c.id === activeRoute || c.slug === activeRoute)
                ?? (activeConversation?.id === activeRoute || activeConversation?.slug === activeRoute
                  ? activeConversation
                  : undefined);
              const targetId = conv?.id ?? activeProduct?.latest_transcript_row_id;
              const isWritable = activeProduct
                ? activeProduct.ordinary_lifecycle === 'open'
                : conv?.archived !== true;
              const chainMembers = conv && !conversations.some(candidate => candidate.id === conv.id)
                ? [...conversations, conv]
                : conversations;
              const computedChainRootId = computeChainRoots(chainMembers).get(conv?.id ?? '');
              const canonicalRootId = activeProduct?.canonical_root.transcript_row_id;
              const chainRootId = canonicalRootId && canonicalRootId !== targetId
                ? canonicalRootId
                : computedChainRootId;
              if (!targetId || !isWritable || (chainRootId != null && !activeProduct)) return undefined;
              return async () => {
                try {
                  if (chainRootId != null) {
                    await api.archiveChain(chainRootId);
                  } else {
                    await api.archiveConversation(targetId);
                  }
                  navigate('/');
                } catch (error) {
                  if (!notifyArchiveCloseConflict(targetId, error)) throw error;
                }
              };
            })()
          : undefined,
      }),
    [navigate, currentSlug, activeProduct, activeConvId, activeConversation, conversations],
  );
  const actionsRef = useRef(actions);
  useEffect(() => {
    actionsRef.current = actions;
  }, [actions]);

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
  const searchScope = state.status === 'open' ? state.scope : null;
  const searchQuery = state.status === 'open' ? state.query : null;

  useEffect(() => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    searchAbortRef.current?.abort();
    searchAbortRef.current = null;

    if (!isOpen || searchMode !== 'search') return;
    const query = searchQuery ?? '';

    if (searchScope === 'conversation-content' && query.trim().length === 0) {
      setState(prev => transition(prev, { type: 'SEARCH_AWAITING_QUERY' }, actionsRef.current));
      return;
    }

    setState(prev => transition(prev, { type: 'SEARCH_DEBOUNCING' }, actionsRef.current));

    debounceRef.current = setTimeout(async () => {
      const controller = new AbortController();
      searchAbortRef.current = controller;

      // Read sources via ref — latest value without making sources a dep.
      // This prevents the 5s conversation-poll from re-aborting in-flight requests.
      const eligibleSources = searchScope === 'conversation-content'
        ? sourcesRef.current.filter(source => source.id === 'conversation-content')
        : searchScope === 'conversation-slugs'
          ? sourcesRef.current.filter(source => source.id === 'conversations')
          : sourcesRef.current.filter(source => source.id !== 'conversation-content');

      setState(prev => transition(prev, { type: 'SEARCH_LOADING' }, actionsRef.current));

      try {
        const allResults = await Promise.all(
          eligibleSources.map(source => source.search(query, controller.signal))
        );

        if (!controller.signal.aborted) {
          setState(prev => transition(prev, { type: 'SET_RESULTS', results: allResults.flat() }, actionsRef.current));
        }
      } catch (error) {
        if (controller.signal.aborted) return;
        if (error instanceof ConversationSearchWarmingError) {
          setState(prev => transition(prev, { type: 'SEARCH_WARMING', message: error.message }, actionsRef.current));
          return;
        }
        setState(prev => transition(prev, {
          type: 'SEARCH_ERROR',
          message: error instanceof Error ? error.message : 'Search failed',
        }, actionsRef.current));
      }
    }, SEARCH_DEBOUNCE_MS);

    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
      searchAbortRef.current?.abort();
    };
  }, [isOpen, searchMode, searchScope, searchQuery]);

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
                const source = sources.find(s => s.id === selected.sourceId);
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
          const source = sources.find(s => s.id === selected.sourceId);
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
          searchStatus={state.searchStatus}
          query={state.query}
          scope={state.scope}
          onHover={handleHover}
          onClick={handleClick}
        />
      </div>
    </div>
  );
}
