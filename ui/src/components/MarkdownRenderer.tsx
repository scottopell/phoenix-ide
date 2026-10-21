import { useMemo, type ComponentProps } from 'react';
import ReactMarkdown from 'react-markdown';
import type { Root } from 'mdast';
import { remarkDetails } from './remarkDetails';
import { remarkConversationTranscriptLinks } from './conversationMarkdownImages';
import { prepareMarkdown, type MarkdownMode } from './prepareMarkdown';
import { MarkdownDetails, MarkdownSummary } from './MarkdownDetails';

const disclosureComponents = { details: MarkdownDetails, summary: MarkdownSummary };

type Props = Omit<ComponentProps<typeof ReactMarkdown>, 'remarkPlugins'> & { mode: MarkdownMode; tree?: Root | undefined };
export function MarkdownRenderer({ mode, tree, children, components, ...props }: Props) {
  const prepared = useMemo(() => tree ?? prepareMarkdown(children ?? '', mode), [tree, children, mode]);
  const plugins = useMemo(() => [
    [remarkDetails, { tree: prepared }],
    ...(mode === 'conversation' ? [remarkConversationTranscriptLinks] : []),
  ] as NonNullable<ComponentProps<typeof ReactMarkdown>['remarkPlugins']>, [prepared, mode]);
  const mapped = useMemo(() => mode === 'conversation' ? { ...components, ...disclosureComponents } : components, [components, mode]);
  return <ReactMarkdown {...props} remarkPlugins={plugins} components={mapped}>{children}</ReactMarkdown>;
}
