import { createContext } from 'react';

export interface PatchContext {
  modifiedLines: Set<number>;
  firstModifiedLine?: number;
}

export interface OpenFileState {
  path: string;
  rootDir: string;
  patchContext?: PatchContext;
}

export interface FileExplorerContextValue {
  /** Open a file in the viewer */
  openFile: (path: string, rootDir: string, patchContext?: PatchContext) => void;
  /** Currently open file, or null */
  activeFile: string | null;
  /** Close the file viewer */
  closeFile: () => void;
  /** Full prose reader state (path + rootDir + patchContext) */
  openFileState: OpenFileState | null;
}

export const FileExplorerContext = createContext<FileExplorerContextValue | null>(null);
