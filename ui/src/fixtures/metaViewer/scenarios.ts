import type {
  CodeViewerPayload,
  HtmlViewerPayload,
  ImageViewerPayload,
  TextViewerPayload,
} from '../../components/viewer/metaViewerTypes';
import { metaViewerScenarioDefinitions } from './types';
import type { MetaViewerScenario, MetaViewerScenarioId } from './types';

// Synthetic, author-neutral root — not a real machine. The viewer scenarios
// only need self-consistent, portable paths, so they must not pin one
// developer's home directory (matching the grounding-panel fixture).
const FIXTURE_HOME = '/home/dev';
const ROOT = `${FIXTURE_HOME}/phoenix-ide`;

const noop = () => {};

function absPath(filePath: string): string {
  return `${ROOT}/${filePath}`;
}

// --- Content blobs -------------------------------------------------------

// A long log: enough lines that production would flip it to the plain-text
// fallback. We set `renderMode: 'plainLargeText'` explicitly rather than relying
// on the FileViewer threshold, so the fixture stays small but exercises the
// fallback body + its banner.
const LARGE_LOG = Array.from({ length: 140 }, (_, i) => {
  const n = String(i + 1).padStart(4, '0');
  return `2026-06-21T17:${String(i % 60).padStart(2, '0')}:12.${n} INFO  worker[${i % 8}] processed batch ${i + 1} (rows=${(i * 37) % 500}, lag=${i % 12}ms)`;
}).join('\n');

const PATCH_FILE = [
  'server:',
  '  host: 127.0.0.1',
  '  port: 8443',
  '  tls: true',
  '  workers: 8',
  'logging:',
  '  level: info',
  '  format: json',
  'cache:',
  '  backend: memory',
  '  max_entries: 4096',
].join('\n');

const NOTES_FILE = [
  'function resolveScope(input) {',
  '  const trimmed = input.trim();',
  '  if (!trimmed) return null;',
  '  const parts = trimmed.split(/\\s+/);',
  '  const head = parts.shift();',
  '  return { head, rest: parts };',
  '}',
].join('\n');

// Lines deliberately far wider than the viewport — including unbreakable tokens
// — to establish the horizontal-overflow user story (does a long line wrap, get
// a horizontal scrollbar, or clip?). Mix of normal lines for contrast.
const LONG_LINES_TEXT = [
  'Build log — long lines, no wrapping.',
  `GET /api/v1/conversations/abcdef0123/grounding?include=tasks,skills,mcp,workscope&fields=${'name,status,priority,'.repeat(18)}end`,
  'short normal line',
  `unbreakable_token_${'x'.repeat(260)}`,
  `A long prose sentence ${'that keeps going and going '.repeat(14)}past the right edge of the viewport.`,
  'tail',
].join('\n');

const LONG_LINES_CODE = [
  "import { resolveScope, classifyInput, normalizePath } from '../../utils/scope';",
  '',
  `const endpoint = 'https://example.com/api/v1/resource?${'param=value&'.repeat(40)}end=1';`,
  '',
  `export const ids = items.filter((i) => i.active).map((i) => i.id)${'.concat(extra)'.repeat(20)};`,
  '',
  'export function describe(input: string): string {',
  `  return input + '${'-'.repeat(300)}'; // trailing comment forced well beyond the viewport width`,
  '}',
].join('\n');

const HTML_DOC = [
  '<!doctype html>',
  '<html lang="en">',
  '  <head>',
  '    <meta charset="utf-8" />',
  '    <title>Release notes</title>',
  '    <style>',
  '      body { font-family: system-ui; margin: 24px; color: #0b3d2e; }',
  '      h1 { color: #0a7b5a; }',
  '    </style>',
  '  </head>',
  '  <body>',
  '    <h1>Sandboxed preview</h1>',
  '    <p>Rendered markup with styles, scripts disabled.</p>',
  '    <ul>',
  '      <li>First item</li>',
  '      <li>Second item</li>',
  '    </ul>',
  '  </body>',
  '</html>',
].join('\n');

// `data:` URL so the sandboxed iframe renders deterministically with no network.
const HTML_PREVIEW_URL = `data:text/html;charset=utf-8,${encodeURIComponent(HTML_DOC)}`;

// Inline SVG (rendered via <img>) so the image body and its fullscreen takeover
// have real, deterministic pixels at any zoom — no binary asset, no network.
const IMAGE_SVG = [
  '<svg xmlns="http://www.w3.org/2000/svg" width="640" height="400">',
  '  <rect width="640" height="400" fill="#10243e"/>',
  '  <rect x="40" y="40" width="560" height="320" fill="none" stroke="#4aa3ff" stroke-width="3"/>',
  '  <circle cx="320" cy="170" r="70" fill="#1f6feb"/>',
  '  <text x="320" y="300" fill="#cfe6ff" font-family="system-ui" font-size="28" text-anchor="middle">diagram.png</text>',
  '</svg>',
].join('');
const IMAGE_URL = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(IMAGE_SVG)}`;

// --- Payload builders ----------------------------------------------------

function textPayload(
  filePath: string,
  content: string,
  extra: Partial<TextViewerPayload> = {},
): TextViewerPayload {
  return {
    kind: 'text',
    title: filePath.split('/').pop() ?? filePath,
    absolutePath: absPath(filePath),
    onClose: noop,
    onSendNotes: noop,
    filePath,
    rootDir: ROOT,
    content,
    ...extra,
  };
}

function codePayload(filePath: string, content: string, language: string): CodeViewerPayload {
  return {
    kind: 'code',
    title: filePath.split('/').pop() ?? filePath,
    absolutePath: absPath(filePath),
    onClose: noop,
    onSendNotes: noop,
    filePath,
    rootDir: ROOT,
    content,
    language,
  };
}

function htmlPayload(filePath: string, content: string): HtmlViewerPayload {
  return {
    kind: 'html',
    title: filePath.split('/').pop() ?? filePath,
    absolutePath: absPath(filePath),
    onClose: noop,
    onSendNotes: noop,
    filePath,
    rootDir: ROOT,
    content,
    language: 'html',
    previewUrl: HTML_PREVIEW_URL,
  };
}

function imagePayload(filePath: string): ImageViewerPayload {
  const fileName = filePath.split('/').pop() ?? filePath;
  return {
    kind: 'image',
    title: fileName,
    absolutePath: absPath(filePath),
    onClose: noop,
    onSendNotes: noop,
    url: IMAGE_URL,
    mimeType: 'image/svg+xml',
    fileName,
  };
}

// --- Per-scenario assembly ----------------------------------------------

// Built once and looked up by id; keeps the canonical definition list (which
// drives the id union + capture set) free of heavy payload objects.
const byId: Record<MetaViewerScenarioId, Omit<MetaViewerScenario, 'id' | 'title' | 'theme' | 'interaction'>> = {
  'large-text-fallback-dark': {
    settleSelector: '[data-testid="viewer-large-text-fallback"]',
    payload: textPayload('logs/worker.log', LARGE_LOG, { renderMode: 'plainLargeText' }),
  },
  'large-text-fallback-light': {
    settleSelector: '[data-testid="viewer-large-text-fallback"]',
    payload: textPayload('logs/worker.log', LARGE_LOG, { renderMode: 'plainLargeText' }),
  },
  'patch-context-dark': {
    settleSelector: '.annotatable--modified',
    payload: textPayload('config/service.yaml', PATCH_FILE, {
      patchContext: { modifiedLines: new Set([3, 4, 5]), firstModifiedLine: 3 },
    }),
  },
  'long-lines-text-dark': {
    // Per-line text body; horizontal overflow is governed by .viewer-text CSS.
    settleSelector: '.viewer-text',
    payload: textPayload('logs/long-lines.txt', LONG_LINES_TEXT),
  },
  'long-lines-code-dark': {
    // Code routes through Pierre's virtualized CodeView, which owns its own
    // horizontal scroll; wait for a rendered line, not just the wrapper.
    settleSelector: '.phoenix-file-codeview [data-line]',
    payload: codePayload('src/longLines.ts', LONG_LINES_CODE, 'typescript'),
  },
  'html-source-dark': {
    settleSelector: '.viewer-code',
    payload: htmlPayload('docs/release-notes.html', HTML_DOC),
  },
  'html-preview-dark': {
    settleSelector: '.viewer-html-preview iframe',
    payload: htmlPayload('docs/release-notes.html', HTML_DOC),
  },
  'image-takeover-dark': {
    settleSelector: '.viewer-shell--takeover',
    payload: imagePayload('docs/diagram.png'),
  },
  'notes-panel-dark': {
    settleSelector: '.notes-panel',
    payload: textPayload('src/resolveScope.txt', NOTES_FILE),
    seedNotes: [
      { lineNumber: 2, lineContent: '  const trimmed = input.trim();', body: 'Guard against non-string input before trim.' },
      { lineNumber: 5, lineContent: '  const head = parts.shift();', body: 'shift() on an empty array is undefined — document the contract.' },
    ],
  },
  'annotation-dialog-dark': {
    settleSelector: '.annotation-overlay',
    payload: textPayload('src/resolveScope.txt', NOTES_FILE),
  },
  'loading-dark': {
    settleSelector: '.viewer-loading',
    loader: { state: 'loading', filePath: 'logs/worker.log', rootDir: ROOT },
  },
  'error-dark': {
    settleSelector: '.viewer-error',
    loader: { state: 'error', filePath: 'assets/font.bin', rootDir: ROOT },
  },
};

export const metaViewerScenarios: MetaViewerScenario[] = metaViewerScenarioDefinitions.map((def) => ({
  ...def,
  ...byId[def.id],
}));

export function getMetaViewerScenario(id: string | null | undefined): MetaViewerScenario {
  return metaViewerScenarios.find((scenario) => scenario.id === id) ?? metaViewerScenarios[0]!;
}
