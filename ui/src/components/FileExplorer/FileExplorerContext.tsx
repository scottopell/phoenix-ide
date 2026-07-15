import { useMemo } from 'react';
import type { ReactNode } from 'react';
import { FileExplorerContext } from './fileExplorerTypes';
import type { OpenFileState } from './fileExplorerTypes';
import { useViewerSlotData, useViewerSlotCommands } from '../../contexts/ViewerSlotContext';

/**
 * Back-compat adapter exposing the file-oriented view of the unified viewer
 * slot. The slot's URL contract, patch context, restoration, and mutex now live
 * in `ViewerSlotProvider` (which must wrap this); this provider just projects
 * the slot into the `{ openFile, closeFile, activeFile, openFileState }`
 * shape that the file explorer panel, command palette, and work actions consume.
 */
export function FileExplorerProvider({ children }: { children: ReactNode }) {
  const slot = useViewerSlotData();
  const { openProse, close } = useViewerSlotCommands();

  const openFileState = useMemo<OpenFileState | null>(() => {
    if (slot.kind !== 'prose') return null;
    const state: OpenFileState = { path: slot.file.path, rootDir: slot.file.rootDir };
    if (slot.patchContext) state.patchContext = slot.patchContext;
    if (slot.file.focus) state.focus = slot.file.focus;
    return state;
  }, [slot]);

  const value = useMemo(() => ({
    openFile: openProse,
    closeFile: close,
    activeFile: openFileState?.path ?? null,
    openFileState,
  }), [openProse, close, openFileState]);

  return (
    <FileExplorerContext.Provider value={value}>
      {children}
    </FileExplorerContext.Provider>
  );
}
