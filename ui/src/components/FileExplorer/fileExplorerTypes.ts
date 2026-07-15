import { createContext } from 'react';
import type { ViewerFocus } from '../viewer/metaViewerTypes';

export interface PatchContext {
  modifiedLines: Set<number>;
  firstModifiedLine?: number;
}

export type OpenFileOptions =
  | { kind: 'patch'; patchContext: PatchContext }
  | { kind: 'line'; lineNumber: number }
  | { kind: 'range'; startLine: number; endLine: number };

export interface OpenFileState {
  path: string;
  rootDir: string;
  patchContext?: PatchContext;
  focus?: ViewerFocus;
}

export interface FileExplorerContextValue {
  /** Open a file in the viewer */
  openFile: (path: string, rootDir: string, options?: OpenFileOptions) => void;
  /** Currently open file, or null */
  activeFile: string | null;
  /** Close the file viewer */
  closeFile: () => void;
  /** Open-file state (path + rootDir + patchContext) */
  openFileState: OpenFileState | null;
}

export const FileExplorerContext = createContext<FileExplorerContextValue | null>(null);
