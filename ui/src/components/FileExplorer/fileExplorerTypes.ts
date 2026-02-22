import { createContext } from 'react';

export interface PatchContext {
  modifiedLines: Set<number>;
  firstModifiedLine?: number;
}

export interface ProseReaderState {
  path: string;
  rootDir: string;
  patchContext?: PatchContext;
}

export interface FileExplorerContextValue {
  /** Open a file in the prose reader */
  openFile: (path: string, rootDir: string, patchContext?: PatchContext) => void;
  /** Currently open file, or null */
  activeFile: string | null;
  /** Close the prose reader */
  closeFile: () => void;
  /** Full prose reader state (path + rootDir + patchContext) */
  proseReaderState: ProseReaderState | null;
}

export const FileExplorerContext = createContext<FileExplorerContextValue | null>(null);
