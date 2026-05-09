import { useCallback, useMemo } from 'react';
import type { ReactNode } from 'react';
import { useSearchParams } from 'react-router-dom';
import { useScopedState } from '../../hooks/useScopedState';
import { FileExplorerContext } from './fileExplorerTypes';
import type { PatchContext, ProseReaderState } from './fileExplorerTypes';

interface FileExplorerProviderProps {
  children: ReactNode;
  /**
   * Scope identifier (typically the active conversation slug). Used to scope
   * the per-conversation `patchContext` state so highlights from one
   * conversation don't bleed into another. The path + rootDir live in URL
   * search params (?file=...&root=...) and are naturally scoped by the
   * `/c/:slug` segment, so they don't need a scopeKey reset.
   */
  scopeKey?: string | undefined;
}

const FILE_PARAM = 'file';
const ROOT_PARAM = 'root';

export function FileExplorerProvider({ children, scopeKey }: FileExplorerProviderProps) {
  const [searchParams, setSearchParams] = useSearchParams();

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
  }, [setPatchContext, setSearchParams]);

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
