import { useCallback, useEffect, useMemo } from 'react';
import type { ReactNode } from 'react';
import { useLocation, useSearchParams } from 'react-router-dom';
import { useScopedState } from '../../hooks/useScopedState';
import { FileExplorerContext } from './fileExplorerTypes';
import type { PatchContext, ProseReaderState } from './fileExplorerTypes';
import {
  clearLastViewer,
  getLastViewer,
  setLastViewer,
} from '../../storage/lastViewerStorage';

interface FileExplorerProviderProps {
  children: ReactNode;
  /**
   * Scope identifier (typically the active conversation slug). Used to scope
   * the per-conversation `patchContext` state so highlights from one
   * conversation don't bleed into another. The path + rootDir live in URL
   * search params (?file=...&root=...) and are naturally scoped by the
   * `/c/:slug` segment, so they don't need a scopeKey reset.
   *
   * Also keys per-conversation last-viewer storage (REQ-VS-014): in-app
   * navigation back to this slug restores its previously open viewer from
   * localStorage.
   */
  scopeKey?: string | undefined;
}

const FILE_PARAM = 'file';
const ROOT_PARAM = 'root';

export function FileExplorerProvider({ children, scopeKey }: FileExplorerProviderProps) {
  const [searchParams, setSearchParams] = useSearchParams();
  const location = useLocation();

  // patchContext (modified-line highlights) is conversation-specific and not
  // URL-encodable (Set<number>). It lives in scoped state so a slug change
  // clears it; opening a fresh URL without patchContext just shows the file
  // without highlights, which is correct.
  const [patchContext, setPatchContext] = useScopedState<PatchContext | null>(
    scopeKey,
    null,
  );

  const file = searchParams.get(FILE_PARAM);
  const root = searchParams.get(ROOT_PARAM);

  const proseReaderState = useMemo<ProseReaderState | null>(() => {
    if (!file || !root) return null;
    const state: ProseReaderState = { path: file, rootDir: root };
    if (patchContext) state.patchContext = patchContext;
    return state;
  }, [file, root, patchContext]);

  const openFile = useCallback(
    (path: string, rootDir: string, nextPatchContext?: PatchContext) => {
      setPatchContext(nextPatchContext ?? null);
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set(FILE_PARAM, path);
          next.set(ROOT_PARAM, rootDir);
          return next;
        },
        { replace: true },
      );
    },
    [setPatchContext, setSearchParams],
  );

  const closeFile = useCallback(() => {
    setPatchContext(null);
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.delete(FILE_PARAM);
        next.delete(ROOT_PARAM);
        return next;
      },
      { replace: true },
    );
    if (scopeKey) clearLastViewer(scopeKey);
  }, [setPatchContext, setSearchParams, scopeKey]);

  // REQ-VS-014: persist the current viewer URL params for this conversation
  // whenever the slot is non-empty. Snapshotting searchParams.toString() keeps
  // future ?viewer=diff / ?viewer=browser additions round-trippable without
  // touching this effect. We depend on the SERIALIZED string (a primitive)
  // rather than the searchParams object reference (which changes every
  // render), so the effect re-fires exactly when the URL actually changes.
  const searchString = searchParams.toString();
  useEffect(() => {
    if (!scopeKey) return;
    if (!file || !root) return;
    setLastViewer(scopeKey, searchString);
  }, [scopeKey, file, root, searchString]);

  // REQ-VS-014: restore the last viewer on in-app entry to a conversation.
  // react-router v6/v7 documented behavior: location.key === 'default' on the
  // initial SPA mount (cold reload, browser refresh, iOS PWA cold start);
  // every subsequent navigate() mints a fresh key. Cold reload is excluded
  // by design (D1) -- the URL is authoritative there. The "URL has no ?file"
  // precondition prevents double-firing on browser back/forward, where the
  // URL already carries the prior params.
  useEffect(() => {
    if (!scopeKey) return;
    if (location.key === 'default') return;
    if (file || root) return;
    const stored = getLastViewer(scopeKey);
    if (!stored) return;
    setSearchParams(new URLSearchParams(stored), { replace: true });
    // file / root included so the effect re-evaluates when a separate code
    // path (e.g. closeFile) clears the URL; we explicitly want NOT to fire
    // in that case because the user just closed it. The guard above handles
    // that: closeFile clears storage, so getLastViewer returns null.
  }, [scopeKey, location.key, file, root, setSearchParams]);

  const activeFile = proseReaderState?.path ?? null;

  const value = useMemo(() => ({
    openFile,
    activeFile,
    closeFile,
    proseReaderState,
  }), [openFile, activeFile, closeFile, proseReaderState]);

  return (
    <FileExplorerContext.Provider value={value}>
      {children}
    </FileExplorerContext.Provider>
  );
}
