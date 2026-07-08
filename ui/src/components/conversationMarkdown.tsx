import React from 'react';

export type MarkdownAnchorProps = React.ComponentPropsWithoutRef<'a'> & { node?: unknown };
export type MarkdownImageProps = React.ComponentPropsWithoutRef<'img'> & { node?: unknown };

export function ConversationMarkdownAnchor({ node, children, ...props }: MarkdownAnchorProps) {
  void node;
  return (
    <a {...props} target="_blank" rel="noopener noreferrer">
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
