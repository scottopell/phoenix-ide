/**
 * FileSource — command palette source that searches files in the active
 * conversation's working directory via the server-side search endpoint.
 *
 * Uses /api/conversations/:id/files/search which is gitignore-aware,
 * recursive, and fuzzy-matches server-side -- the same endpoint as the
 * ./ inline file reference autocomplete in InputArea.
 *
 * Returns empty results when query is empty (no point listing every file).
 * Returns empty results when no convId is available (root route).
 */
import { api } from '../../../api';
import type { OpenFileOptions } from '../../FileExplorer/fileExplorerTypes';
import type { PaletteSource, PaletteItem } from '../types';

export function createFileSource(
  convId: string,
  rootDir: string,
  openFile: (path: string, rootDir: string, options?: OpenFileOptions) => void,
): PaletteSource {
  return {
    id: 'files',
    category: 'Files',

    async search(query: string, signal?: AbortSignal): Promise<PaletteItem[]> {
      if (!query.trim()) return [];
      try {
        const result = await api.searchConversationFiles(convId, query, 50, signal);
        // Quick-open is a viewer entry point: offer only files the viewer can
        // open, the same verdict the sidebar dispatches on. Opaque files would
        // route into the `/api/files/read` 400 path.
        return result.items
          .filter(entry => entry.viewer.kind !== 'opaque')
          .map(entry => toItem(entry.path, rootDir));
      } catch (err) {
        if (err instanceof Error && err.name === 'AbortError') return [];
        return [];
      }
    },

    onSelect(item: PaletteItem) {
      const relPath = item.metadata as string;
      const absPath = relPath.startsWith('/') ? relPath : `${rootDir}/${relPath}`;
      openFile(absPath, rootDir);
    },
  };
}

function toItem(relPath: string, rootDir: string): PaletteItem {
  const parts = relPath.split('/');
  const name = parts[parts.length - 1] ?? relPath;
  const dir = parts.length > 1 ? parts.slice(0, -1).join('/') : rootDir;
  return {
    id: relPath,
    title: name,
    subtitle: dir,
    category: 'Files',
    metadata: relPath,
  };
}
