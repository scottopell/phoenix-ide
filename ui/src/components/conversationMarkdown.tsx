import React from 'react';
import { FilePathLink } from '../utils/FilePathLink';
import { isConversationFilePath, type FilePathCopyContext } from '../utils/linkify';

export type MarkdownAnchorProps = React.ComponentPropsWithoutRef<'a'> & {
  node?: unknown;
  onFileClick?: ((filePath: string) => void) | undefined;
  filePathCopyContext?: FilePathCopyContext | undefined;
};
export type MarkdownImageProps = React.ComponentPropsWithoutRef<'img'> & { node?: unknown };

export function ConversationMarkdownAnchor({
  node,
  children,
  href,
  onFileClick,
  filePathCopyContext,
  ...props
}: MarkdownAnchorProps) {
  void node;
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
