import { useState, useCallback, useMemo } from 'react';
import type { ReactNode } from 'react';
import { FileExplorerContext } from './fileExplorerTypes';
import type { PatchContext, ProseReaderState } from './fileExplorerTypes';

interface FileExplorerProviderProps {
  children: ReactNode;
  /**
   * Scope identifier (typically the active conversation slug). When this
   * changes, the open file is closed so the viewer never shows a file from
   * the previous scope. `undefined` is a single shared scope.
   *
   * Reset is synchronous ("adjusting state during render" pattern, see
   * https://react.dev/learn/you-might-not-need-an-effect#adjusting-some-state-when-a-prop-changes)
   * so the first render after a scope change already has the cleared state
   * — no flash of cross-scope content.
   */
  scopeKey?: string | undefined;
}

export function FileExplorerProvider({ children, scopeKey }: FileExplorerProviderProps) {
  const [proseReaderState, setProseReaderState] = useState<ProseReaderState | null>(null);
  const [trackedScope, setTrackedScope] = useState<string | undefined>(scopeKey);

  if (trackedScope !== scopeKey) {
    // Adjust state during render: React applies this and re-renders before
    // commit, so children never see the old proseReaderState under the new
    // scopeKey.
    setTrackedScope(scopeKey);
    if (proseReaderState !== null) setProseReaderState(null);
  }

  const openFile = useCallback((path: string, rootDir: string, patchContext?: PatchContext) => {
    const state: ProseReaderState = { path, rootDir };
    if (patchContext) state.patchContext = patchContext;
    setProseReaderState(state);
  }, []);

  const closeFile = useCallback(() => {
    setProseReaderState(null);
  }, []);

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
