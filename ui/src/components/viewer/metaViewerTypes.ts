/**
 * MetaViewer payload contract.
 *
 * A `MetaViewerPayload` is a *resolved, renderable* view of content — the
 * loader (FileViewer for files) has already fetched bytes and classified the
 * render kind. MetaViewer routes a payload to exactly one body renderer; it
 * never fetches. This is the typed boundary the diff-renderer replacement
 * plugs into: a new diff body becomes a renderer swap behind this shape, not
 * another architectural change.
 *
 * Image is a first-class payload kind here, not a special case bolted onto the
 * loader — every viewable thing flows through the same router.
 */

export type TextRenderMode = 'rich' | 'plainLargeText';

export interface PatchContext {
  modifiedLines: Set<number>;
  firstModifiedLine?: number | undefined;
}

interface CommonPayload {
  /** Header title — typically the file name. */
  title: string;
  /**
   * Stable viewer identity. Absolute path for files; used as the key for
   * scroll restoration and file-scoped review notes.
   */
  absolutePath: string;
  onClose: () => void;
  onSendNotes: (notes: string) => void;
  /** Initial search/jump target line. */
  focusLine?: number | undefined;
  /** Initial inclusive range to shade after opening from a ranged tool read. */
  focusRange?: { startLine: number; endLine: number } | undefined;
  /** Render inline (desktop split-pane) instead of as an overlay. */
  inline?: boolean | undefined;
}

/** Shared shape for the four annotatable text render kinds. */
interface TextLikePayload extends CommonPayload {
  filePath: string;
  rootDir: string;
  content: string;
  renderMode?: TextRenderMode | undefined;
  patchContext?: PatchContext | undefined;
}

export interface MarkdownViewerPayload extends TextLikePayload {
  kind: 'markdown';
}

export interface CodeViewerPayload extends TextLikePayload {
  kind: 'code';
  /** Syntax-highlighter grammar identifier. */
  language: string;
}

export interface TextViewerPayload extends TextLikePayload {
  kind: 'text';
}

export interface HtmlViewerPayload extends TextLikePayload {
  kind: 'html';
  /** Syntax-highlighter grammar for source mode (`'html'`). */
  language: string;
  /** Sandboxed-preview + open-in-browser URL (`/preview<absolutePath>`). */
  previewUrl: string;
}

export interface ImageViewerPayload extends CommonPayload {
  kind: 'image';
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

/** Payloads whose bodies render annotatable lines and carry file review notes. */
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
