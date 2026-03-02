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

interface CommandPaletteProps {
  conversations: Conversation[];
}

export function CommandPalette({ conversations }: CommandPaletteProps) {
  const [state, setState] = useState<PaletteState>(initialState);
  const [isDesktop, setIsDesktop] = useState(() => window.matchMedia('(min-width: 1025px)').matches);
  const navigate = useNavigate();
  const location = useLocation();
  const overlayRef = useRef<HTMLDivElement>(null);
  // Track selected index for mouse hover without triggering state machine
  const [hoverIndex, setHoverIndex] = useState<number | null>(null);
  const { openFile } = useFileExplorer();

  // Desktop detection
  useEffect(() => {
    const mq = window.matchMedia('(min-width: 1025px)');
    const handler = (e: MediaQueryListEvent) => setIsDesktop(e.matches);
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  }, []);

  // Extract current slug from URL
  const slugMatch = location.pathname.match(/^\/c\/(.+)$/);
  const currentSlug = slugMatch?.[1] ?? null;
  const activeConversation = useMemo(
    () => conversations.find(c => c.slug === currentSlug) ?? null,
    [conversations, currentSlug],
  );
  const activeCwd = activeConversation?.cwd ?? null;

  // Build sources and actions
  const sources: PaletteSource[] = useMemo(
    () => [
      createConversationSource(conversations, (slug) => {
        navigate(`/c/${slug}`);
      }),
      ...(activeCwd
        ? [createFileSource(activeCwd, (path, rootDir) => openFile(path, rootDir))]
        : []),
    ],
    [conversations, navigate, activeCwd, openFile],
  );

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

  const context = useMemo(() => ({ sources, actions }), [sources, actions]);

  // Dispatch helper
  const dispatch = useCallback(
    (event: Parameters<typeof transition>[1]) => {
      setState(prev => transition(prev, event, context));
      setHoverIndex(null);
    },
    [context],
  );

  // Global Cmd/Ctrl+P shortcut (REQ-CP-001)
  useEffect(() => {
    if (!isDesktop) return;

    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'p') {
        e.preventDefault();
        e.stopPropagation();
        setState(prev => {
          if (prev.status === 'open') {
            return transition(prev, { type: 'CLOSE' }, context);
          }
          return transition(prev, { type: 'OPEN' }, context);
        });
      }
    };

    window.addEventListener('keydown', handler, true);
    return () => window.removeEventListener('keydown', handler, true);
  }, [isDesktop, context]);

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
