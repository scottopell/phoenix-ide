import type { Root } from 'mdast';
import type { Plugin } from 'unified';
import type {} from 'remark-parse';
import { interpretMarkdown } from './markdown-disclosures/interpret';
import { basicOptions } from './markdown-disclosures/support';
import type { MarkdownConfiguration, MarkdownInterpretation, SourceRange } from './markdown-disclosures/types';

export { interpretMarkdown } from './markdown-disclosures/interpret';
export { basicOptions, gfmOptions } from './markdown-disclosures/support';
export type {
  DisclosureDetails, DisclosureSummary, InterpretationStats,
  MarkdownConfiguration, MarkdownInterpretation, SourceRange,
} from './markdown-disclosures/types';

export interface RemarkDetailsOptions {
  /** A root selected from the canonical interpretation, for chunk rendering. */
  tree?: Root;
}

/** Install after remark-parse. This is a parser, not a reparsing transformer. */
export const remarkDetails: Plugin<[options?: RemarkDetailsOptions], Root> = function remarkDetails(options) {
  this.parser = (source) => {
    if (options?.tree) return options.tree;
    return interpretMarkdown(source, {
      ...this.data('settings'),
      extensions: this.data('micromarkExtensions') ?? [],
      mdastExtensions: this.data('fromMarkdownExtensions') ?? [],
    }).tree;
  };
};

export function markdownDetailsRanges(
  input: string | MarkdownInterpretation,
  options: MarkdownConfiguration = basicOptions,
): SourceRange[] {
  return (typeof input === 'string' ? interpretMarkdown(input, options) : input).ranges;
}

/**
 * Typed replacement for the old content-to-string closure. Definitions remain
 * AST nodes; callers render/select this interpretation instead of appending
 * definition source to arbitrary chunks.
 */
export function markdownReferenceContext(
  input: string | MarkdownInterpretation,
  options: MarkdownConfiguration = basicOptions,
): MarkdownInterpretation {
  return typeof input === 'string' ? interpretMarkdown(input, options) : input;
}
