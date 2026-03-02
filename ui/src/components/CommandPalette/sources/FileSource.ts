/**
 * FileSource — command palette source that searches files in the conversation's cwd
 * and opens the selected file in ProseReader via fileExplorer.openFile.
 */
import type { PaletteSource, PaletteItem } from '../types';
import { fuzzyMatch } from '../fuzzyMatch';

export function createFileSource(
  rootPath: string,
  openFile: (path: string, rootDir: string) => void,
): PaletteSource {
  // Cache of loaded file paths for this rootPath
  let cached: string[] = [];
  let cacheKey = '';

  async function loadFiles(root: string): Promise<string[]> {
    if (cacheKey === root && cached.length > 0) return cached;
    try {
      const resp = await fetch(`/api/files/list?path=${encodeURIComponent(root)}`);
      if (!resp.ok) return [];
      const data = await resp.json();
      // Flatten: collect all file paths recursively via repeated listing
      const files: string[] = [];
      const walk = async (items: { name: string; path: string; is_dir: boolean }[]) => {
        for (const item of items) {
          if (!item.is_dir) {
            files.push(item.path);
          }
        }
      };
      await walk(data.files || []);
      cacheKey = root;
      cached = files;
      return files;
    } catch {
      return [];
    }
  }

  // Kick off initial load immediately
  void loadFiles(rootPath);

  return {
    id: 'files',
    category: 'Files',

    search(query: string): PaletteItem[] {
      if (!query) return [];
      // Reload in background if rootPath changed
      if (cacheKey !== rootPath) void loadFiles(rootPath);
      const items = cached.map(p => toItem(p, rootPath));
      return fuzzyMatch(items, query, item => item.title).slice(0, 15);
    },

    onSelect(item: PaletteItem) {
      const path = item.metadata as string;
      openFile(path, rootPath);
    },
  };
}

function toItem(filePath: string, rootPath: string): PaletteItem {
  // Show path relative to rootPath for readability
  const rel = filePath.startsWith(rootPath + '/')
    ? filePath.slice(rootPath.length + 1)
    : filePath;
  const parts = rel.split('/');
  const name = parts[parts.length - 1];
  const dir = parts.slice(0, -1).join('/');

  return {
    id: filePath,
    title: name,
    subtitle: dir || rootPath,
    category: 'Files',
    metadata: filePath,
  };
}
