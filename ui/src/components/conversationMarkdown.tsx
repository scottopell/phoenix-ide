import React from 'react';
import { Link, useLocation } from 'react-router-dom';
import { FilePathLink } from '../utils/FilePathLink';
import { isConversationFilePath, type FilePathCopyContext } from '../utils/linkify';
import { coordinatorConversationLinkState } from './conversationReturnOrigin';

export type MarkdownAnchorProps = React.ComponentPropsWithoutRef<'a'> & {
  node?: unknown;
  onFileClick?: ((filePath: string) => void) | undefined;
  filePathCopyContext?: FilePathCopyContext | undefined;
};
export type MarkdownImageProps = React.ComponentPropsWithoutRef<'img'> & { node?: unknown };

function isPhoenixConversationDestination(path: string): boolean {
  return /^\/(?:c|chains|global)(?:\/|$)/.test(path);
}

function appLocalDestination(href: string | undefined): string | undefined {
  if (!href || typeof window === 'undefined') return undefined;
  if (href.startsWith('/') && !href.startsWith('//')) {
    return isPhoenixConversationDestination(href) ? href : undefined;
  }
  if (!/^[a-z][a-z0-9+.-]*:/i.test(href)) return undefined;
  try {
    const destination = new URL(href);
    return destination.origin === window.location.origin && isPhoenixConversationDestination(destination.pathname)
      ? `${destination.pathname}${destination.search}${destination.hash}`
      : undefined;
  } catch {
    return undefined;
  }
}

export function ConversationMarkdownAnchor({
  node,
  children,
  href,
  onFileClick,
  filePathCopyContext,
  ...props
}: MarkdownAnchorProps) {
  void node;
  const location = useLocation();
  const appDestination = appLocalDestination(href);
  if (appDestination) {
    return (
      <Link
        {...props}
        to={appDestination}
        state={coordinatorConversationLinkState(
          `${location.pathname}${location.search}${location.hash}`,
          appDestination,
        )}
      >
        {children}
      </Link>
    );
  }
  if (href && isConversationFilePath(href)) {
    return (
      <FilePathLink
        filePath={href}
        onFileClick={onFileClick}
        filePathCopyContext={filePathCopyContext}
      >
        {children}
      </FilePathLink>
    );
  }
  return (
    <a {...props} href={href} target="_blank" rel="noopener noreferrer">
      {children}
    </a>
  );
}

export function ConversationMarkdownImage({ node, src, alt, ...props }: MarkdownImageProps) {
  void node;
  return (
    <img
      {...props}
      className={[props.className, 'conversation-markdown-image'].filter(Boolean).join(' ')}
      src={src}
      alt={alt ?? ''}
      loading="lazy"
      decoding="async"
    />
  );
}
