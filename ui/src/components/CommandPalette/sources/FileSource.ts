/**
 * FileSource — command palette source that searches files in the active
 * conversation's working directory via the server-side search endpoint.
 *
 * Uses /api/conversations/:id/files/search which is gitignore-aware,
 * recursive, and fuzzy-matches server-side — the same endpoint as the
 * ./ inline file reference autocomplete in InputArea.
 *
 * Returns empty results when query is empty (no point listing every file).
 * Returns empty results when no convId is available (root route).
 */
import { api } from '../../../api';
import type { PaletteSource, PaletteItem } from '../types';

export function createFileSource(
  convId: string,
  rootDir: string,
  openFile: (path: string, rootDir: string) => void,
): PaletteSource {
  return {
    id: 'files',
    category: 'Files',

    async search(query: string, signal?: AbortSignal): Promise<PaletteItem[]> {
      if (!query.trim()) return [];
      try {
        const result = await api.searchConversationFiles(convId, query, 15, signal);
        return result.items.map(entry => toItem(entry.path, rootDir));
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
  const name = parts[parts.length - 1];
  const dir = parts.length > 1 ? parts.slice(0, -1).join('/') : rootDir;
  return {
    id: relPath,
    title: name,
    subtitle: dir,
    category: 'Files',
    metadata: relPath,
  };
}
