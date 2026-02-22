import { useContext } from 'react';
import { FileExplorerContext } from '../components/FileExplorer/fileExplorerTypes';
import type { FileExplorerContextValue } from '../components/FileExplorer/fileExplorerTypes';

export type { FileExplorerContextValue };

export function useFileExplorer(): FileExplorerContextValue {
  const ctx = useContext(FileExplorerContext);
  if (!ctx) {
    throw new Error('useFileExplorer must be used within FileExplorerProvider');
  }
  return ctx;
}
