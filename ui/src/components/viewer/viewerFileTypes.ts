/**
 * Viewer file classification.
 *
 * Two distinct concerns previously both derived "what kind of file is this"
 * from the path extension:
 *
 *   - the server's `/api/files/read` `file_type` (markdown | code | config |
 *     text | image | data | unknown), used for explorer icons and the
 *     text/binary gate, and
 *   - ProseReader's private `getFileType`, used to pick a render mode.
 *
 * They are not the same taxonomy: the render path needs an `html` split (for
 * the source/preview toggle) that the server folds into `code`, and a
 * syntax-highlighter `language` the server has no concept of. This module is
 * the single client-side owner of the *render* classification. It trusts the
 * server bucket when one is available and only layers the html split and
 * language on top, so the extension→bucket table is not duplicated.
 */

/** How a text file's body is rendered inside the viewer. */
export type ViewerRenderKind = 'markdown' | 'html' | 'code' | 'text';

/**
 * Server `file_type` values from `detect_file_type` (api/handlers.rs).
 * `image` never reaches a text render kind — it resolves to an image payload
 * upstream — but is included so the mapping is total over the server enum.
 */
export type ServerFileType =
  | 'markdown'
  | 'code'
  | 'config'
  | 'text'
  | 'image'
  | 'data'
  | 'unknown'
  | 'folder';

function extensionOf(path: string): string {
  // Mirror `path.split('.').pop()` semantics: no dot → whole string, which is
  // not a known extension and falls through to the text/unknown default.
  const base = path.split('/').pop() ?? path;
  if (!base.includes('.')) return '';
  return base.split('.').pop()?.toLowerCase() ?? '';
}

const HTML_EXTENSIONS = new Set(['html', 'htm']);

/** Extensions the renderer syntax-highlights when no server bucket says so. */
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
 * Map a server `file_type` bucket to a render kind. `html` is never produced
 * here — the html split is an extension refinement applied by the caller —
 * `config` highlights as code, and `text`/`data`/`unknown` render as plain
 * lines.
 */
function renderKindForServerType(fileType: ServerFileType): ViewerRenderKind {
  switch (fileType) {
    case 'markdown':
      return 'markdown';
    case 'code':
      return 'code';
    case 'config':
      return 'code';
    case 'text':
    case 'data':
    case 'unknown':
    case 'image':
    case 'folder':
      return 'text';
  }
}

/** Extension-only render kind, used when the server bucket is unavailable. */
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
 * Classify a text file for rendering. When `serverFileType` is provided (from
 * `/api/files/read`), it is the authority for the markdown/code/config/text
 * distinction; an html extension still overrides to `html` so the source/
 * preview toggle is available. Without it, classification falls back to the
 * extension table, preserving the pre-dedup behaviour exactly.
 */
export function classifyViewerFile(
  path: string,
  serverFileType?: ServerFileType | string | undefined,
): ViewerClassification {
  const ext = extensionOf(path);
  const language = LANGUAGE_BY_EXTENSION[ext] ?? 'text';

  if (HTML_EXTENSIONS.has(ext)) {
    return { renderKind: 'html', language };
  }

  if (serverFileType && isServerFileType(serverFileType)) {
    return { renderKind: renderKindForServerType(serverFileType), language };
  }

  return { renderKind: renderKindForExtension(ext), language };
}

function isServerFileType(value: string): value is ServerFileType {
  return (
    value === 'markdown' || value === 'code' || value === 'config' ||
    value === 'text' || value === 'image' || value === 'data' ||
    value === 'unknown' || value === 'folder'
  );
}
