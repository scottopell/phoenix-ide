import type { Definition, Parent, PhrasingContent, Root, RootContent } from 'mdast';
import type { Options } from 'mdast-util-from-markdown/phoenix';

export type MarkdownConfiguration = Readonly<Options>;
export interface SourceRange {
  start: number;
  end: number;
}

export interface DisclosureSummary extends Parent {
  type: 'disclosureSummary';
  children: PhrasingContent[];
  data: { hName: 'summary'; hProperties: Record<string, never> };
}

export interface DisclosureDetails extends Parent {
  type: 'disclosureDetails';
  children: [DisclosureSummary, ...RootContent[]];
  data: { hName: 'details'; hProperties: { open?: true } };
}

declare module 'mdast' {
  interface RootContentMap {
    disclosureDetails: DisclosureDetails;
    disclosureSummary: DisclosureSummary;
  }
  interface BlockContentMap {
    disclosureDetails: DisclosureDetails;
  }
}

export interface InterpretationStats {
  indexed: number;
  validatedUnits: number;
  horizonBytes: number;
  documentBytes: number;
  dispatch: number;
  sessions: number;
  reused: number;
  frameSteps: number;
  recognitionDefinitionsProbes: number;
  recognitionDefinitionsPushes: number;
  recognitionFootnotesProbes: number;
  recognitionFootnotesPushes: number;
  definitionProbes: number;
  definitionPushes: number;
  footnoteProbes: number;
  footnotePushes: number;
  submittedUnits: number;
  submissions: number;
  committedEvents: number;
}

export interface MarkdownInterpretation {
  tree: Root;
  ranges: SourceRange[];
  /** Source-ordered, first-wins definitions; these are nodes in tree, not copies. */
  definitions: Definition[];
  stats: InterpretationStats;
}

export interface AcceptedFrame extends SourceRange {
  open: boolean;
  summaryOpenStart: number;
  summaryStart: number;
  summaryEnd: number;
  bodyStart: number;
  bodyEnd: number;
}

export type TagGrammar =
  | { valid: false }
  | { valid: true; closing: boolean; kind: 'details' | 'summary'; open: boolean };

export type RecognizedTag = SourceRange & { grammar: TagGrammar };
export interface Recognition {
  candidates: Map<number, number>;
  tags: RecognizedTag[];
  accepted: AcceptedFrame[];
  ranges: SourceRange[];
  stats: InterpretationStats;
}
