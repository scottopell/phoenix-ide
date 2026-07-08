import { createElement } from 'react';
import { defaultUrlTransform, type Components } from 'react-markdown';
import { ConversationMarkdownAnchor, ConversationMarkdownImage } from './conversationMarkdown';

const SAFE_DATA_IMAGE = /^data:image\/(?:png|jpe?g|gif|webp|bmp|avif);base64,/i;

function trimTrailingSlash(path: string): string {
  return path.endsWith('/') ? path.slice(0, -1) : path;
}

function encodePathSegment(segment: string): string {
  try {
    return encodeURIComponent(decodeURIComponent(segment));
  } catch {
    return encodeURIComponent(segment);
  }
}

function encodedPreviewUrlForAbsolutePath(path: string): string {
  return `/preview${path.split('/').map(encodePathSegment).join('/')}`;
}

function conversationMarkdownUrlTransform(url: string, key: string): string | null | undefined {
  const trimmed = url.trim();
  if (key === 'src' && (/^blob:/i.test(trimmed) || SAFE_DATA_IMAGE.test(trimmed))) {
    return trimmed;
  }
  return defaultUrlTransform(url);
}

export function resolveConversationMarkdownImageSrc(src: string | undefined, rootDir?: string | undefined): string | undefined {
  if (!src) return src;
  const trimmed = src.trim();
  if (!trimmed) return undefined;

  if (/^(?:https?:|blob:)/i.test(trimmed) || trimmed.startsWith('//') || SAFE_DATA_IMAGE.test(trimmed)) {
    return trimmed;
  }

  if (/^[a-z][a-z0-9+.-]*:/i.test(trimmed)) {
    return undefined;
  }

  if (trimmed === '/preview' || trimmed.startsWith('/preview/')) {
    return trimmed;
  }

  if (trimmed.startsWith('/')) {
    return rootDir ? encodedPreviewUrlForAbsolutePath(trimmed) : undefined;
  }

  if (!rootDir) return undefined;
  const relative = trimmed.replace(/^\.\//, '');
  return encodedPreviewUrlForAbsolutePath(`${trimTrailingSlash(rootDir)}/${relative}`);
}

export interface ConversationMarkdownContext {
  rootDir?: string | undefined;
}

export function createConversationMarkdownComponents(ctx: ConversationMarkdownContext = {}): Components {
  return {
    a: ConversationMarkdownAnchor,
    img: ({ src, ...props }) => createElement(ConversationMarkdownImage, {
      ...props,
      src: resolveConversationMarkdownImageSrc(src, ctx.rootDir),
    }),
  } satisfies Components;
}

export const CONVERSATION_MARKDOWN_COMPONENTS: Components = createConversationMarkdownComponents();

export const CONVERSATION_MARKDOWN_URL_TRANSFORM = conversationMarkdownUrlTransform;
