/**
 * MetaViewer payload contract.
 *
 * A `MetaViewerPayload` is a *resolved, renderable* view of content — the
 * loader (`FileViewer` for files, `MessageViewer` for conversation messages)
 * has already fetched or resolved the text and classified the render kind.
 * MetaViewer routes a payload to exactly one body renderer; it never fetches.
 *
 * Source-specific capabilities and identities are carried on discriminated
 * `source` variants so invalid combinations (such as a message carrying file
 * paths, patch context, HTML preview URLs, or file-only body kinds) are
 * structurally unrepresentable.
 */

export type TextRenderMode = 'rich' | 'plainLargeText';

export type ViewerFocus =
  | { kind: 'line'; lineNumber: number }
  | { kind: 'range'; startLine: number; endLine: number };

export interface PatchContext {
  modifiedLines: Set<number>;
  firstModifiedLine?: number | undefined;
}

/** Minimal filesystem identity shared by all file-backed viewer payloads. */
export interface FileIdentitySource {
  kind: 'file';
  absolutePath: string;
}

/** Filesystem source for text-like payloads (markdown, code, text, html). */
export interface FileTextSource extends FileIdentitySource {
  filePath: string;
  rootDir: string;
  renderMode?: TextRenderMode | undefined;
  patchContext?: PatchContext | undefined;
  focus?: ViewerFocus | undefined;
}

/** Conversation message source — markdown only; cannot carry file capabilities. */
export interface MessageViewerSource {
  kind: 'message';
  sequenceId: number;
  messageId?: string | undefined;
  occurrenceToken?: string | undefined;
}

export type MarkdownViewerSource = FileTextSource | MessageViewerSource;
export type ViewerSource = FileIdentitySource | FileTextSource | MessageViewerSource;

interface CommonPayload {
  /** Header title — file name or message label. */
  title: string;
  onClose: () => void;
  /** Optional send callback; omitted when composer capability is unavailable. */
  onSendNotes?: ((notes: string) => void | Promise<void>) | undefined;
  /** Presentation selected by the URL-derived viewer slot. */
  presentation?: 'pane' | 'fullscreen' | undefined;
  /** Render inline (desktop split-pane) instead of as an overlay. */
  inline?: boolean | undefined;
  /** Whether a wide desktop pane is available as the alternate presentation. */
  canTogglePresentation?: boolean | undefined;
  onPresentationChange?: ((presentation: 'pane' | 'fullscreen') => void) | undefined;
}

export interface MarkdownViewerPayload extends CommonPayload {
  kind: 'markdown';
  source: MarkdownViewerSource;
  content: string;
}

export interface CodeViewerPayload extends CommonPayload {
  kind: 'code';
  source: FileTextSource;
  content: string;
  /** Syntax-highlighter grammar identifier. */
  language: string;
}

export interface TextViewerPayload extends CommonPayload {
  kind: 'text';
  source: FileTextSource;
  content: string;
}

export interface HtmlViewerPayload extends CommonPayload {
  kind: 'html';
  source: FileTextSource;
  content: string;
  /** Syntax-highlighter grammar for source mode (`'html'`). */
  language: string;
  /** Sandboxed-preview + open-in-browser URL (`/preview<absolutePath>`). */
  previewUrl: string;
}

export interface ImageViewerPayload extends CommonPayload {
  kind: 'image';
  source: FileIdentitySource;
  url: string;
  mimeType: string;
  fileName: string;
}

export type MetaViewerPayload =
  | MarkdownViewerPayload
  | CodeViewerPayload
  | TextViewerPayload
  | HtmlViewerPayload
  | ImageViewerPayload;

/** Payloads whose bodies render annotatable lines and carry review notes. */
export type TextLikeViewerPayload =
  | MarkdownViewerPayload
  | CodeViewerPayload
  | TextViewerPayload
  | HtmlViewerPayload;

export function isTextLikePayload(
  payload: MetaViewerPayload,
): payload is TextLikeViewerPayload {
  return payload.kind !== 'image';
}

export function isFileTextSource(
  source: ViewerSource,
): source is FileTextSource {
  return source.kind === 'file' && 'filePath' in source;
}

/** Stable identity string for find surfaces, scroll keys, and state resets. */
export function viewerSourceIdentity(source: ViewerSource): string {
  if (source.kind === 'file') {
    return `file:${source.absolutePath}`;
  }
  return `message:${source.occurrenceToken ?? source.messageId ?? String(source.sequenceId)}`;
}

/** Source-aware localStorage key for scroll restoration (no fake message paths). */
export function viewerScrollKey(source: ViewerSource): string {
  if (source.kind === 'file') {
    return `phoenix:prose-scroll:${source.absolutePath}`;
  }
  return `phoenix:message-scroll:${source.occurrenceToken ?? source.messageId ?? String(source.sequenceId)}`;
}
