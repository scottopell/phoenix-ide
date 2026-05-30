import { useMemo } from 'react';
import type { ReactNode } from 'react';
import { FileExplorerContext } from './fileExplorerTypes';
import type { ProseReaderState } from './fileExplorerTypes';
import { useViewerSlot } from '../../contexts/ViewerSlotContext';

/**
 * Back-compat adapter exposing the file-oriented view of the unified viewer
 * slot. The slot's URL contract, patch context, restoration, and mutex now live
 * in `ViewerSlotProvider` (which must wrap this); this provider just projects
 * the slot into the `{ openFile, closeFile, activeFile, proseReaderState }`
 * shape that the file explorer panel, command palette, and work actions consume.
 */
export function FileExplorerProvider({ children }: { children: ReactNode }) {
  const { slot, openProse, close } = useViewerSlot();

  const proseReaderState = useMemo<ProseReaderState | null>(() => {
    if (slot.kind !== 'prose') return null;
    const state: ProseReaderState = { path: slot.file.path, rootDir: slot.file.rootDir };
    if (slot.patchContext) state.patchContext = slot.patchContext;
    return state;
  }, [slot]);

  const value = useMemo(() => ({
    openFile: openProse,
    closeFile: close,
    activeFile: proseReaderState?.path ?? null,
    proseReaderState,
  }), [openProse, close, proseReaderState]);

  return (
    <FileExplorerContext.Provider value={value}>
      {children}
    </FileExplorerContext.Provider>
  );
}
