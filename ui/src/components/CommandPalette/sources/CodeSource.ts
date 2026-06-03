import { api } from '../../../api';
import type { CodeSearchEntry } from '../../../api';
import type { OpenFileOptions } from '../../FileExplorer/fileExplorerTypes';
import type { PaletteSource, PaletteItem } from '../types';

interface CodeSearchMetadata {
  relPath: string;
  lineNumber: number;
}

export function createCodeSource(
  convId: string,
  rootDir: string,
  openFile: (path: string, rootDir: string, options?: OpenFileOptions) => void,
): PaletteSource {
  return {
    id: 'code',
    category: 'Code',

    async search(query: string, signal?: AbortSignal): Promise<PaletteItem[]> {
      if (!query.trim()) return [];
      try {
        const result = await api.searchConversationCode(convId, query, 50, signal);
        return result.items.map(entry => toItem(entry));
      } catch (err) {
        if (err instanceof Error && err.name === 'AbortError') return [];
        return [];
      }
    },

    onSelect(item: PaletteItem) {
      const meta = item.metadata as CodeSearchMetadata;
      const absPath = meta.relPath.startsWith('/') ? meta.relPath : `${rootDir}/${meta.relPath}`;
      openFile(absPath, rootDir, { kind: 'line', lineNumber: meta.lineNumber });
    },
  };
}

function toItem(entry: CodeSearchEntry): PaletteItem {
  const parts = entry.path.split('/');
  const name = parts[parts.length - 1] ?? entry.path;
  const dir = parts.length > 1 ? parts.slice(0, -1).join('/') : '.';
  const matchedText = sliceByCharSpan(entry.line_text, entry.match_start, entry.match_end);
  return {
    id: `${entry.path}:${entry.line_number}:${entry.match_start}`,
    title: matchedText || entry.line_text.trim() || name,
    subtitle: `${dir}/${name}:${entry.line_number}`,
    snippet: entry.line_text,
    category: 'Code',
    metadata: { relPath: entry.path, lineNumber: entry.line_number } satisfies CodeSearchMetadata,
  };
}

function sliceByCharSpan(text: string, start: number, end: number): string {
  return Array.from(text).slice(start, end).join('');
}
