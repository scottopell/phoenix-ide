import { useEffect, useMemo, useState } from 'react';
import { AlertCircle, Loader2 } from 'lucide-react';
import { ViewerShell } from './viewer/ViewerShell';
import { MetaViewer } from './viewer/MetaViewer';
import { classifyViewerFile } from './viewer/viewerFileTypes';
import type { MetaViewerPayload, PatchContext, TextRenderMode } from './viewer/metaViewerTypes';
import type { TextCategory } from '../generated/TextCategory';

/**
 * FileViewer — the file loader/adapter.
 *
 * Resolves a "view this file" request into a renderable `MetaViewerPayload`:
 * fetches `/api/files/read`, then classifies the result into a markdown / code /
 * text / html / image payload (trusting the server's `file_type` bucket). The
 * actual rendering is delegated to `MetaViewer`. Loading and error states are
 * owned here — they are not resolved payloads, so they never reach MetaViewer.
 */

type ReadFileResult =
  | { kind: 'text'; content: string; encoding: string; category: TextCategory }
  | { kind: 'image'; mime_type: string; url: string };

export interface FileViewerProps {
  filePath: string;
  rootDir: string;
  onClose: () => void;
  onSendNotes: (notes: string) => void;
  patchContext?: PatchContext | undefined;
  focusLine?: number | undefined;
  focusRange?: { startLine: number; endLine: number } | undefined;
  /** Render inline (no overlay) for desktop split-pane mode. */
  inline?: boolean | undefined;
}

async function readFile(path: string): Promise<ReadFileResult> {
  const response = await fetch(`/api/files/read?path=${encodeURIComponent(path)}`);
  if (!response.ok) {
    const error = await response.json().catch(() => ({ error: 'Unknown error' }));
    throw new Error(error.error || 'Failed to read file');
  }
  const data = await response.json();
  if (data.kind === 'image') {
    return { kind: 'image', mime_type: data.mime_type, url: data.url };
  }
  return {
    kind: 'text',
    content: data.content,
    encoding: data.encoding ?? 'utf-8',
    category: data.category ?? 'unknown',
  };
}

export function FileViewer({
  filePath,
  rootDir,
  onClose,
  onSendNotes,
  patchContext,
  focusLine,
  focusRange,
  inline,
}: FileViewerProps) {
  const [fileData, setFileData] = useState<ReadFileResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const absolutePath = useMemo(() => {
    if (filePath.startsWith('/')) return filePath;
    return rootDir.endsWith('/') ? rootDir + filePath : rootDir + '/' + filePath;
  }, [filePath, rootDir]);
  const fileName = filePath.split('/').pop() || filePath;

  useEffect(() => {
    let cancelled = false;
    async function load() {
      setLoading(true);
      setError(null);
      setFileData(null);
      try {
        const result = await readFile(absolutePath);
        if (!cancelled) setFileData(result);
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : 'Failed to load file');
      } finally {
        if (!cancelled) setLoading(false);
      }
    }
    load();
    return () => { cancelled = true; };
  }, [absolutePath]);

  if (fileData) {
    const payload = buildPayload(fileData, {
      filePath,
      rootDir,
      absolutePath,
      fileName,
      onClose,
      onSendNotes,
      ...(patchContext !== undefined ? { patchContext } : {}),
      ...(focusLine !== undefined ? { focusLine } : {}),
      ...(focusRange !== undefined ? { focusRange } : {}),
      ...(inline !== undefined ? { inline } : {}),
    });
    return <MetaViewer payload={payload} />;
  }

  return (
    <ViewerShell
      mode={inline ? 'inline' : 'overlay'}
      ariaLabel={`File viewer: ${fileName}`}
      title={fileName}
      titleTooltip={absolutePath}
      noteCount={0}
      onToggleNotes={() => undefined}
      onSend={() => undefined}
      onClose={onClose}
    >
      <div className="viewer-content">
        {loading ? (
          <div className="viewer-loading">
            <Loader2 size={32} className="spinning" />
            <span>Loading file...</span>
          </div>
        ) : (
          <div className="viewer-error">
            <AlertCircle size={32} />
            <span>{error ?? 'Failed to load file'}</span>
            <button onClick={onClose}>Close</button>
          </div>
        )}
      </div>
    </ViewerShell>
  );
}

// content.length is UTF-16 code units, not bytes — the threshold is a
// character count, named accordingly.
const LARGE_TEXT_CHARS = 250_000;
const LARGE_TEXT_LINES = 2_000;

function textRenderMode(content: string): TextRenderMode {
  if (content.length > LARGE_TEXT_CHARS) return 'plainLargeText';
  let lines = 1;
  for (let i = 0; i < content.length; i += 1) {
    if (content.charCodeAt(i) === 10) {
      lines += 1;
      if (lines > LARGE_TEXT_LINES) return 'plainLargeText';
    }
  }
  return 'rich';
}

interface PayloadContext {
  filePath: string;
  rootDir: string;
  absolutePath: string;
  fileName: string;
  onClose: () => void;
  onSendNotes: (notes: string) => void;
  patchContext?: PatchContext | undefined;
  focusLine?: number | undefined;
  focusRange?: { startLine: number; endLine: number } | undefined;
  inline?: boolean | undefined;
}

function buildPayload(data: ReadFileResult, ctx: PayloadContext): MetaViewerPayload {
  const common = {
    title: ctx.fileName,
    absolutePath: ctx.absolutePath,
    onClose: ctx.onClose,
    onSendNotes: ctx.onSendNotes,
    ...(ctx.focusLine !== undefined ? { focusLine: ctx.focusLine } : {}),
    ...(ctx.focusRange !== undefined ? { focusRange: ctx.focusRange } : {}),
    ...(ctx.inline !== undefined ? { inline: ctx.inline } : {}),
  };

  if (data.kind === 'image') {
    return { kind: 'image', ...common, url: data.url, mimeType: data.mime_type, fileName: ctx.fileName };
  }

  const { renderKind, language } = classifyViewerFile(ctx.filePath, data.category);
  // Code and plain text render through Pierre's virtualized CodeView, which stays
  // responsive on large files, so they never need the plain-text fallback.
  // Markdown and HTML source still build line-per-node DOM, so they keep the guard.
  const renderMode = renderKind === 'code' || renderKind === 'text' ? 'rich' : textRenderMode(data.content);
  const textCommon = {
    ...common,
    filePath: ctx.filePath,
    rootDir: ctx.rootDir,
    content: data.content,
    ...(renderMode !== 'rich' ? { renderMode } : {}),
    ...(ctx.patchContext !== undefined ? { patchContext: ctx.patchContext } : {}),
  };

  switch (renderKind) {
    case 'markdown':
      return { kind: 'markdown', ...textCommon };
    case 'code':
      return { kind: 'code', ...textCommon, language };
    case 'html':
      return { kind: 'html', ...textCommon, language, previewUrl: `/preview${ctx.absolutePath}` };
    case 'text':
      return { kind: 'text', ...textCommon };
  }
}
