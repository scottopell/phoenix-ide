import { gfm } from 'micromark-extension-gfm';
import { gfmFromMarkdown } from 'mdast-util-gfm';
import type { Nodes } from 'mdast';
import type { Construct } from 'micromark-util-types';
import type { InterpretationStats, MarkdownConfiguration, RecognizedTag } from './types';

export const basicOptions: MarkdownConfiguration = Object.freeze({ extensions: [], mdastExtensions: [] });
export const gfmOptions: MarkdownConfiguration = Object.freeze({ extensions: [gfm()], mdastExtensions: [gfmFromMarkdown()] });

export function createStats(length: number): InterpretationStats {
  return {
    indexed: length, validatedUnits: 0, horizonBytes: 0, documentBytes: 0, dispatch: 0,
    sessions: 0, reused: 0, frameSteps: 0,
    recognitionDefinitionsProbes: 0, recognitionDefinitionsPushes: 0,
    recognitionFootnotesProbes: 0, recognitionFootnotesPushes: 0,
    definitionProbes: 0, definitionPushes: 0, footnoteProbes: 0, footnotePushes: 0,
    submittedUnits: 0, submissions: 0, committedEvents: 0,
  };
}

/** Native parsers use an array's push/includes pair for definition membership. */
export function membership(
  stats: InterpretationStats,
  key: 'recognitionDefinitions' | 'recognitionFootnotes' | 'definition' | 'footnote',
): string[] {
  const values: string[] = [];
  const index = new Set<string>();
  Object.defineProperties(values, {
    includes: { value(value: string, from = 0) {
      stats[`${key}Probes`]++;
      if (from !== 0) throw new Error('Unsupported native membership offset');
      return index.has(value);
    } },
    push: { value(...items: string[]) {
      for (const item of items) {
        stats[`${key}Pushes`]++;
        index.add(item);
        Array.prototype.push.call(values, item);
      }
      return values.length;
    } },
  });
  return values;
}

export function walk(root: Nodes, visit: (node: Nodes) => void): void {
  const stack: Nodes[] = [root];
  while (stack.length) {
    const node = stack.pop()!;
    visit(node);
    if ('children' in node) {
      for (let i = node.children.length - 1; i >= 0; i--) stack.push(node.children[i]!);
    }
  }
}

/** ConstructRecord also admits singular entries in the upstream declarations. */
export function constructList(value: Construct | Construct[] | undefined): Construct[] {
  return value === undefined ? [] : Array.isArray(value) ? value : [value];
}

export function append<T>(target: T[], items: readonly T[]): void {
  for (const item of items) target.push(item);
}

export function tagGrammar(
  source: string, start: number, prefixEnd: number, quoteEnds: ReadonlyMap<number, number>,
): RecognizedTag {
  const closing = source[start + 1] === '/';
  const kind = source[start + (closing ? 2 : 1)]!.toLowerCase() === 'd' ? 'details' : 'summary';
  let i = prefixEnd;
  let open = false;
  const space = (c: string | undefined) => c !== undefined && /[\t\n\r\f ]/.test(c);
  const invalid = (): RecognizedTag => ({ start, end: i, grammar: { valid: false } });
  const valid = (): RecognizedTag => ({ start, end: i + 1, grammar: { valid: true, closing, kind, open } });
  if (closing) {
    while (space(source[i])) i++;
    return source[i] === '>' ? valid() : invalid();
  }
  while (i < source.length) {
    if (source[i] === '>') return valid();
    if (!space(source[i])) return invalid();
    while (space(source[i])) i++;
    if (source[i] === '>') continue;
    const begin = i;
    if (!/[A-Za-z_:]/.test(source[i] ?? '')) break;
    i++;
    while (i < source.length && /[A-Za-z0-9_.:-]/.test(source[i]!)) i++;
    const attribute = source.slice(begin, i).toLowerCase();
    const end = i;
    while (space(source[i])) i++;
    if (source[i] === '=') {
      i++;
      while (space(source[i])) i++;
      const quote = source[i];
      if (quote === '"' || quote === "'") {
        const quoteEnd = quoteEnds.get(i);
        if (quoteEnd === undefined) break;
        i = quoteEnd + 1;
      } else {
        const valueStart = i;
        while (i < source.length && !space(source[i]) && !/["'`=<>]/.test(source[i]!)) i++;
        if (i === valueStart) break;
      }
    } else i = end;
    if (attribute === 'open') open = true;
  }
  return invalid();
}
