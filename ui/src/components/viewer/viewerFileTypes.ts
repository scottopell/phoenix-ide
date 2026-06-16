/**
 * Viewer render classification.
 *
 * Openability and the text/image split are decided once on the server
 * (`FileViewerKind`) and never re-derived here. What remains is purely a
 * *render* concern for text files: which render mode (markdown / html / code /
 * plain) and which syntax-highlighter language to use. The server's
 * `TextCategory` is the authority for the markdown/code/config/plain
 * distinction; this module only layers on the `html` split (for the source/
 * preview toggle) and a syntax language, neither of which the server models.
 */

import type { TextCategory } from '../../generated/TextCategory';

/** How a text file's body is rendered inside the viewer. */
export type ViewerRenderKind = 'markdown' | 'html' | 'code' | 'text';

function extensionOf(path: string): string {
  // Mirror `path.split('.').pop()` semantics: no dot → whole string, which is
  // not a known extension and falls through to the text/unknown default.
  const base = path.split('/').pop() ?? path;
  if (!base.includes('.')) return '';
  return base.split('.').pop()?.toLowerCase() ?? '';
}

const HTML_EXTENSIONS = new Set(['html', 'htm']);

/** Extensions the renderer syntax-highlights when no server category says so. */
const CODE_EXTENSIONS = new Set([
  'rs', 'ts', 'tsx', 'js', 'jsx', 'py', 'go', 'java', 'cpp', 'c', 'h', 'hpp',
  'css', 'vue', 'svelte', 'php', 'rb', 'swift', 'kt', 'scala',
  'sh', 'bash', 'zsh', 'json', 'yaml', 'yml', 'toml', 'xml', 'sql', 'graphql',
]);

const LANGUAGE_BY_EXTENSION: Record<string, string> = {
  rs: 'rust', ts: 'typescript', tsx: 'tsx', js: 'javascript', jsx: 'jsx',
  py: 'python', go: 'go', java: 'java', cpp: 'cpp', c: 'c', h: 'c', hpp: 'cpp',
  css: 'css', html: 'html', htm: 'html', vue: 'vue', svelte: 'svelte',
  php: 'php', rb: 'ruby', swift: 'swift', kt: 'kotlin', scala: 'scala',
  sh: 'bash', bash: 'bash', zsh: 'bash', json: 'json', yaml: 'yaml', yml: 'yaml',
  toml: 'toml', xml: 'xml', sql: 'sql', graphql: 'graphql', md: 'markdown',
};

/**
 * Map a server `TextCategory` to a render kind. `html` is never produced here —
 * the html split is an extension refinement applied by the caller — `config`
 * highlights as code, and `plain`/`unknown` render as plain lines.
 */
function renderKindForCategory(category: TextCategory): ViewerRenderKind {
  switch (category) {
    case 'markdown':
      return 'markdown';
    case 'code':
    case 'config':
      return 'code';
    case 'plain':
    case 'unknown':
      return 'text';
  }
}

/** Extension-only render kind, used when no server category is available. */
function renderKindForExtension(ext: string): ViewerRenderKind {
  if (!ext) return 'text';
  if (ext === 'md' || ext === 'markdown') return 'markdown';
  if (HTML_EXTENSIONS.has(ext)) return 'html';
  if (CODE_EXTENSIONS.has(ext)) return 'code';
  return 'text';
}

export interface ViewerClassification {
  renderKind: ViewerRenderKind;
  /** Syntax-highlighter grammar identifier; `'text'` for no highlighting. */
  language: string;
}

/**
 * Classify a text file for rendering. When `category` is provided (from
 * `/api/files/read`), it is the authority for the markdown/code/text
 * distinction; an html extension still overrides to `html` so the source/
 * preview toggle is available. Without it, classification falls back to the
 * extension table.
 */
export function classifyViewerFile(
  path: string,
  category?: TextCategory | undefined,
): ViewerClassification {
  const ext = extensionOf(path);
  const language = LANGUAGE_BY_EXTENSION[ext] ?? 'text';

  if (HTML_EXTENSIONS.has(ext)) {
    return { renderKind: 'html', language };
  }

  if (category) {
    return { renderKind: renderKindForCategory(category), language };
  }

  return { renderKind: renderKindForExtension(ext), language };
}
