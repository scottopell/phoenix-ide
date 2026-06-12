import React from 'react';
import type { Components } from 'react-markdown';

export type MarkdownAnchorProps = React.ComponentPropsWithoutRef<'a'> & { node?: unknown };

export function ConversationMarkdownAnchor({ node, children, ...props }: MarkdownAnchorProps) {
  void node;
  return (
    <a {...props} target="_blank" rel="noopener noreferrer">
      {children}
    </a>
  );
}

export const CONVERSATION_MARKDOWN_COMPONENTS = {
  a: ConversationMarkdownAnchor,
} satisfies Components;
