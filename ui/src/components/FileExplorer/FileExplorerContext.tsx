import { useState, useCallback, useMemo } from 'react';
import type { ReactNode } from 'react';
import { FileExplorerContext } from './fileExplorerTypes';
import type { PatchContext, ProseReaderState } from './fileExplorerTypes';

export function FileExplorerProvider({ children }: { children: ReactNode }) {
  const [proseReaderState, setProseReaderState] = useState<ProseReaderState | null>(null);

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
