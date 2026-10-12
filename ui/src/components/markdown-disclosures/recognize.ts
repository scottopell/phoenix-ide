import {
  createTokenizer, document as documentInit, parse,
  type BorrowedControl, type ControlledContext, type PhoenixParseContext, type WritableControl,
} from 'micromark/phoenix';
import { content as nativeContent, blankLine } from 'micromark-core-commonmark';
import { factorySpace } from 'micromark-factory-space';
import { markdownLineEnding } from 'micromark-util-character';
import { resolveAll } from 'micromark-util-resolve-all';
import type { Construct, InitialConstruct, State, Token } from 'micromark-util-types';
import { basicOptions, constructList, createStats, membership, tagGrammar } from './support';
import { indexSource } from './source-index';
import { acceptFrames } from './frames';
import type { MarkdownConfiguration, Recognition, RecognizedTag, SourceRange, TagAdmission } from './types';

type SessionMode = 'entry' | 'classify' | 'documentClassify' | 'horizon';
type Horizon = SourceRange & { type: 'horizon' };
type NativeOwner = { kind: 'flow'; horizon?: Horizon } | { kind: 'paragraph'; horizon: Horizon };
type LexicalFrame = { admission: TagAdmission; phase: 'summaryOpen' | 'summary' | 'body' };
type SessionResult =
  | Horizon
  | (SourceRange & { type: 'marker' | 'native' | 'eof' })
  | { type: 'rootFlow' | 'content'; start: number }
  | { type: 'container'; token: Token };

export function recognizeDisclosures(source: string, options: MarkdownConfiguration = basicOptions): Recognition {
  const stats = createStats(source.length);
  const { tape, candidates, quoteEnds, grammarEnds, pointAt } = indexSource(source);
  const tags: RecognizedTag[] = [];
  const defined = membership(stats, 'recognitionDefinitions');
  const footnotes = membership(stats, 'recognitionFootnotes');
  const lexicalFrames: LexicalFrame[] = [];
  let owner: NativeOwner = { kind: 'flow' };
  let inSummary = false;

  function addTag(start: number, nativeEnd: number, admission: TagAdmission): number {
    const horizon = Math.min(nativeEnd, grammarEnds.get(start)!);
    const tag = tagGrammar(source, start, candidates.get(start)!, quoteEnds, horizon);
    stats.validatedUnits += tag.end - tag.start;
    candidates.set(start, tag.end);
    const grammar = tag.grammar;
    tags.push({ ...tag, admission });
    if (grammar.valid) {
      if (grammar.kind === 'details') {
        if (grammar.closing) {
          if (lexicalFrames.pop()?.admission === 'rootFlow') owner = { kind: 'flow', horizon: owner.horizon };
        } else lexicalFrames.push({ admission, phase: 'summaryOpen' });
      } else if (lexicalFrames.length) {
        const frame = lexicalFrames.at(-1)!;
        if (!grammar.closing && frame.phase === 'summaryOpen') frame.phase = 'summary';
        if (grammar.closing && frame.phase === 'summary') frame.phase = 'body';
      }
    }
    const current = lexicalFrames.at(-1);
    inSummary = current?.admission === 'rootFlow' && current.phase === 'summary';
    return tag.end;
  }

  function nativeSession(start: number, mode: SessionMode): Exclude<SessionResult, { type: 'container' }> {
    stats.sessions++;
    const outcome: { result?: SessionResult } = {};
    let context: ControlledContext | undefined;
    const depth = () => context?.containerDepth ?? 0;
    const control: BorrowedControl = { tape, stop: false };
    const flowControls: WritableControl[] = [];
    const stop = (result: SessionResult): void => {
      if (!outcome.result) {
        outcome.result = result;
        control.stop = true;
        for (const child of flowControls) child.stop = true;
      }
    };
    const marker: Construct = {
      name: 'disclosureBoundary',
      tokenize(_effects, ok, nok) {
        const at = this.now().offset;
        if (depth() || !candidates.has(at)) return nok;
        if (this.interrupt) return ok;
        return () => {
          stop({ type: 'marker', start: at, end: candidates.get(at)! });
          return () => undefined;
        };
      },
    };
    const parser = parse({ ...options, extensions: [...(options.extensions ?? []), { flow: { 60: marker } }] });
    parser.defined = defined;
    parser.gfmFootnotes = footnotes;
    if (mode === 'entry') parser.onRootFlow = (point) => {
      stop({ type: 'rootFlow', start: point.offset });
      return true;
    };
    const content: Construct = {
      ...nativeContent,
      resolve(events, nativeContext) {
        if (depth()) return nativeContent.resolve!(events, nativeContext);
        const token = events.find((event) => event[0] === 'enter' && event[1].type === 'content')![1];
        stats.horizonBytes += token.end.offset - token.start.offset;
        stop({ type: 'horizon', start: token.start.offset, end: token.end.offset });
        return events;
      },
      tokenize(effects, ok, nok) {
        if (!depth() && mode !== 'horizon') return () => {
          stop({ type: 'content', start: this.now().offset });
          return () => undefined;
        };
        return nativeContent.tokenize.call(this, effects, ok, nok);
      },
    };
    const flow: InitialConstruct = {
      tokenize(effects) {
        const now = () => this.now();
        const isTable = () => this.currentConstruct?.name === 'table';
        const clearConstruct = () => { this.currentConstruct = undefined; };
        let begin = 0;
        let tableStart: number | undefined;
        const initial = effects.attempt(blankLine, blank, effects.attempt(
          parser.constructs.flowInitial, after,
          factorySpace(effects, effects.attempt(parser.constructs.flow, after,
            effects.attempt(parser.constructs.contentInitial, after, effects.attempt(content, after))), 'linePrefix'),
        ));
        return first;

        function first(code: Parameters<State>[0]): ReturnType<State> {
          begin = now().offset;
          return initial(code);
        }
        function blank(code: Parameters<State>[0]): ReturnType<State> {
          if (tableStart !== undefined) {
            stop({ type: 'native', start: tableStart, end: now().offset });
            return () => undefined;
          }
          if (code === null) {
            effects.consume(code);
            return undefined;
          }
          effects.enter('lineEndingBlank');
          effects.consume(code);
          effects.exit('lineEndingBlank');
          clearConstruct();
          return first;
        }
        function after(code: Parameters<State>[0]): ReturnType<State> {
          const table = isTable();
          if (table && !depth() && tableStart === undefined) tableStart = begin;
          if (!table && !depth() && !outcome.result) {
            stop({ type: 'native', start: begin, end: now().offset });
            return () => undefined;
          }
          clearConstruct();
          if (code === null) {
            effects.consume(code);
            return undefined;
          }
          if (!markdownLineEnding(code)) return first(code);
          effects.enter('lineEnding');
          effects.consume(code);
          effects.exit('lineEnding');
          return first;
        }
      },
    };
    if (mode === 'classify') context = createTokenizer(parser, flow, pointAt(start), control);
    else {
      parser.flow = (from) => {
        const childControl: WritableControl = { stop: false };
        flowControls.push(childControl);
        return createTokenizer(parser, flow, from, childControl);
      };
      control.onContainerExit = (token) => stop({ type: 'container', token });
      context = createTokenizer(parser, documentInit, pointAt(start), control);
    }
    context.run();
    if (mode !== 'classify') stats.documentBytes += context.now().offset - start;
    const result = outcome.result;
    if (result?.type === 'container') {
      return { type: 'native', start: result.token.start.offset, end: result.token.end.offset };
    }
    return result ?? { type: 'eof', start, end: source.length };
  }

  function inline(start: number, end: number): number {
    const parser = parse(options);
    parser.gfmFootnotes = footnotes;
    const lexicalParser = Object.create(parser) as PhoenixParseContext;
    // Syntax shielding must not depend on whether a real definition has appeared.
    lexicalParser.defined = Object.defineProperty([], 'includes', { value: () => true });
    const control: BorrowedControl = { tape, end, stop: false };
    let yielded: number | undefined;
    const initialize: InitialConstruct = {
      tokenize(effects) {
        const now = () => this.now();
        const previousMatches = (construct: Construct) =>
          !construct.previous || construct.previous.call(this, this.previous);
        return dispatch;
        function dispatch(code: Parameters<State>[0]): ReturnType<State> {
          stats.dispatch++;
          if (code === null) {
            effects.consume(code);
            return undefined;
          }
          if (candidates.has(now().offset)) {
            yielded = now().offset;
            control.stop = true;
            return dispatch;
          }
          const eligible = constructList(parser.constructs.text[code]).some(previousMatches);
          return eligible ? effects.attempt(parser.constructs.text, dispatch, data)(code) : data(code);
        }
        function data(code: Parameters<State>[0]): ReturnType<State> {
          effects.enter('data');
          effects.consume(code);
          effects.exit('data');
          return dispatch;
        }
      },
    };
    const context = createTokenizer(lexicalParser, initialize, pointAt(start), control);
    stats.sessions++;
    context.run();
    resolveAll(Object.values(parser.constructs.text).flatMap(constructList), context.events, context);
    return yielded ?? end;
  }

  let cursor = source.charCodeAt(0) === 65279 ? 1 : 0;
  while (cursor < source.length) {
    if (owner.kind === 'paragraph' && cursor >= owner.horizon.end) owner = { kind: 'flow' };
    const lineStart = cursor === 0 || source[cursor - 1] === '\n' || source[cursor - 1] === '\r';
    let result: Exclude<SessionResult, { type: 'container' }>;
    if (owner.kind === 'paragraph') {
      stats.reused++;
      result = { ...owner.horizon, start: cursor };
    } else {
      result = inSummary
        ? { type: 'horizon', start: cursor, end: source.length }
        : nativeSession(cursor, lineStart ? 'documentClassify' : 'entry');
    }
    if (result.type === 'rootFlow') result = nativeSession(result.start, 'classify');
    if (result.type === 'content' && !lineStart && pointAt(result.start).line !== pointAt(cursor).line) {
      result = nativeSession(result.start, 'documentClassify');
    }
    if (result.type === 'content') {
      const cached = owner.horizon;
      if (cached && result.start >= cached.start && result.start < cached.end) {
        stats.reused++;
        result = { ...cached, start: result.start };
      } else result = nativeSession(result.start, 'horizon');
      if (result.type === 'horizon') owner = { kind: 'paragraph', horizon: result };
    }
    if (result.type === 'horizon') {
      const end = inline(result.start, result.end);
      if (end === result.start && !candidates.has(end)) throw new Error('No inline progress');
      cursor = end;
      const tagEnd = candidates.get(cursor);
      if (tagEnd !== undefined) {
        cursor = addTag(cursor, result.end, 'inline');
      }
    } else if (result.type === 'marker') {
      cursor = addTag(result.start, source.length, 'rootFlow');
    } else if (result.type === 'native') {
      if (result.end <= cursor) throw new Error('No native progress');
      cursor = result.end;
    } else if (result.type === 'eof') cursor = result.end;
    else throw new Error(`Unresolved native session: ${result.type}`);
  }
  return { candidates, tags, ...acceptFrames(source, tags, stats), stats };
}
