import { describe, expect, it } from 'vitest';
import type { ContentBlock, ConversationState, Message } from '../../api';
import type { QueuedMessage } from '../../hooks/useMessageQueue';
import type { RenderUnit } from '../../conversation/renderUnits';
import {
  buildAgentTextFragments,
  buildPatchOutputProjection,
  buildReadFileOutputProjection,
  buildConversationSearchProjection,
  buildDiffSearchProjection,
  buildFileSearchProjection,
  buildMarkdownDisplayBlocks,
  buildSubAgentCardFragments,
  buildTerminalToolResultProjection,
} from './searchProjections';

function userMsg(id: string, text = 'hi', files?: Array<{ original_name: string }>): Message {
  return {
    message_id: id,
    sequence_id: 1,
    conversation_id: 'c1',
    message_type: 'user',
    content: { text, files },
    created_at: '',
  } as Message;
}

function agentMsg(id: string, blocks: ContentBlock[] = []): Message {
  return {
    message_id: id,
    sequence_id: 2,
    conversation_id: 'c1',
    message_type: 'agent',
    content: blocks,
    created_at: '',
  };
}

function toolMsg(id: string, tool_use_id: string, content: Partial<Message['content']> & { content?: string; result?: string; error?: string }): Message {
  return {
    message_id: id,
    sequence_id: 3,
    conversation_id: 'c1',
    message_type: 'tool',
    content: { tool_use_id, ...content },
    created_at: '',
  } as Message;
}

function systemMsg(id: string, text: string): Message {
  return {
    message_id: id,
    sequence_id: 4,
    conversation_id: 'c1',
    message_type: 'system',
    content: { text },
    created_at: '',
  } as Message;
}

function skillMsg(id: string, text: string): Message {
  return {
    message_id: id,
    sequence_id: 5,
    conversation_id: 'c1',
    message_type: 'skill',
    content: { text },
    created_at: '',
  } as Message;
}

function queued(localId: string, text: string, files?: QueuedMessage['files']): QueuedMessage {
  return {
    localId,
    conversationId: 'c1',
    text,
    images: [],
    ...(files ? { files } : {}),
    timestamp: 0,
    status: 'pending',
  };
}

describe('buildFileSearchProjection', () => {
  it('projects file content into 1-based line targets and match ranges', () => {
    const projection = buildFileSearchProjection('alpha\nBeta alpha\n', 'alpha');
    expect(projection.sources.map((source) => ({ lineNumber: source.lineNumber, text: source.text }))).toEqual([
      { lineNumber: 1, text: 'alpha' },
      { lineNumber: 2, text: 'Beta alpha' },
      { lineNumber: 3, text: '' },
    ]);
    expect(projection.matches.map((match) => match.target)).toEqual([
      { kind: 'file-line', lineNumber: 1, startColumn: 0, endColumn: 5, matchOrdinal: 0 },
      { kind: 'file-line', lineNumber: 2, startColumn: 5, endColumn: 10, matchOrdinal: 1 },
    ]);
  });
});

const COMMITTED = [
  'diff --git a/src/foo.ts b/src/foo.ts',
  '--- a/src/foo.ts',
  '+++ b/src/foo.ts',
  '@@ -1,3 +1,4 @@',
  ' shared',
  '-before',
  '+after',
  ' tail',
].join('\n');

const UNCOMMITTED = [
  'diff --git a/src/bar.ts b/src/bar.ts',
  'rename from src/old-bar.ts',
  'rename to src/bar.ts',
  '--- a/src/old-bar.ts',
  '+++ b/src/bar.ts',
  '@@ -1,2 +1,2 @@',
  ' shared',
  '-legacy',
  '+modern',
].join('\n');

describe('buildDiffSearchProjection', () => {
  it('orders committed then uncommitted filenames, headers, and displayed lines', () => {
    const projection = buildDiffSearchProjection(COMMITTED, UNCOMMITTED, 'src');
    expect(projection.sources.filter((s) => s.kind === 'file-header').map((s) => `${s.section}:${s.text}`)).toEqual([
      'committed:src/foo.ts',
      'uncommitted:src/bar.ts',
    ]);
    expect(projection.matches.map((match) => match.target.itemId)).toEqual([
      'committed:src/foo.ts',
      'uncommitted:src/bar.ts',
    ]);
  });

  it('preserves section/file/side identity and de-dupes context lines shown in both panes', () => {
    const projection = buildDiffSearchProjection(COMMITTED, UNCOMMITTED, 'shared');
    expect(projection.sources.filter((source) => source.text === 'shared')).toHaveLength(2);
    expect(projection.matches.map((match) => match.target)).toEqual([
      {
        kind: 'diff-line',
        section: 'committed',
        filePath: 'src/foo.ts',
        itemId: 'committed:src/foo.ts',
        side: 'additions',
        lineNumber: 1,
        startColumn: 0,
        endColumn: 6,
      },
      {
        kind: 'diff-line',
        section: 'uncommitted',
        filePath: 'src/bar.ts',
        itemId: 'uncommitted:src/bar.ts',
        side: 'additions',
        lineNumber: 1,
        startColumn: 0,
        endColumn: 6,
      },
    ]);
  });

  it('includes visible commit-log lines in diff search results', () => {
    const projection = buildDiffSearchProjection(COMMITTED, '', 'hello', 'abc123 hello\ndef456');
    expect(projection.matches).toHaveLength(1);
    expect(projection.matches[0]?.target.itemId).toBe('commit-log:0');
  });

  it('keeps changed lines side-aware', () => {
    const projection = buildDiffSearchProjection(COMMITTED, UNCOMMITTED, 'before');
    expect(projection.matches.map((match) => match.target)).toEqual([
      {
        kind: 'diff-line',
        section: 'committed',
        filePath: 'src/foo.ts',
        itemId: 'committed:src/foo.ts',
        side: 'deletions',
        lineNumber: 2,
        startColumn: 0,
        endColumn: 6,
      },
    ]);
  });

  it('walks parsed hunk rows so context after insertions is only indexed once', () => {
    const diff = [
      'diff --git a/src/demo.ts b/src/demo.ts',
      '--- a/src/demo.ts',
      '+++ b/src/demo.ts',
      '@@ -1,2 +1,3 @@',
      ' shared',
      '+inserted',
      ' tail',
    ].join('\n');

    const projection = buildDiffSearchProjection(diff, '', 'tail');
    expect(projection.sources.filter((source) => source.text === 'tail')).toHaveLength(1);
    expect(projection.matches).toHaveLength(1);
    expect(projection.matches[0]?.target).toEqual({
      kind: 'diff-line',
      section: 'committed',
      filePath: 'src/demo.ts',
      itemId: 'committed:src/demo.ts',
      side: 'additions',
      lineNumber: 3,
      startColumn: 0,
      endColumn: 4,
    });
  });
});

describe('typed semantic display projections', () => {
  it('parses Markdown into stable visible-text blocks without source punctuation', () => {
    const blocks = buildMarkdownDisplayBlocks('# **Plan**\n\nUse `alpha` and ![diagram text](diagram.png).\n\n```ts\nconst beta = 1;\n```');
    expect(blocks.map((block) => block.searchableText)).toEqual(expect.arrayContaining([
      'Plan',
      'Use alpha and diagram text.',
      'const beta = 1;',
    ]));
    expect(blocks.some((block) => block.searchableText.includes('**'))).toBe(false);
    expect(blocks.some((block) => block.searchableText.includes('diagram.png'))).toBe(false);
    expect(blocks.every((block) => block.sourceRange.end > block.sourceRange.start)).toBe(true);
  });

  it('normalizes structured terminal fields instead of indexing JSON punctuation', () => {
    const projection = buildTerminalToolResultProjection(
      'tmux',
      JSON.stringify({ status: 'exited', stdout: 'semantic output', stderr: '' }),
      undefined,
      { toolUseId: 'tmux-1' },
    );
    expect(projection.fullText).toContain('stdout: semantic output');
    expect(projection.fullText).not.toContain('{');
    expect(projection.fragments[0].revealTarget).toMatchObject({
      kind: 'tool-result-terminal',
      family: 'tmux',
      toolUseId: 'tmux-1',
    });
  });

  it('includes renderer-visible terminal status and exit labels', () => {
    const bash = buildTerminalToolResultProjection(
      'bash',
      JSON.stringify({ status: 'kill_pending_kernel', exit_code: 1, truncated_before: true }),
      undefined,
    );
    expect(bash.fullText).toContain('kill pending (kernel)');
    expect(bash.fullText).toContain('[output truncated before this view]');

    const tmux = buildTerminalToolResultProjection(
      'tmux',
      JSON.stringify({ status: 'exited', exit_code: 1, truncated: true }),
      undefined,
    );
    expect(tmux.fullText).toContain('exit code 1');
    expect(tmux.fullText).toContain('[output truncated]');
  });

  it('skips numbered read_file metadata while retaining legacy plain lines', () => {
    const numbered = buildReadFileOutputProjection(
      '     7\talpha\n     8\tbeta\n\n[12 more lines not shown. Use offset=9 to continue.]\n',
      { path: 'src/foo.ts', offset: 7, limit: 2 },
    );
    const numberedLines = numbered.fragments.filter((fragment) => fragment.kind === 'line');
    expect(numberedLines.map((fragment) => fragment.semanticText)).toEqual(['7\talpha', '8\tbeta']);
    expect(numbered.fullText).not.toContain('more lines not shown');

    const legacy = buildReadFileOutputProjection('plain alpha\nplain beta\n', { path: 'legacy.txt', offset: 4 });
    expect(legacy.fragments.filter((fragment) => fragment.kind === 'line').map((fragment) => fragment.semanticText)).toEqual([
      '4\tplain alpha',
      '5\tplain beta',
    ]);
  });

  it('keeps complete sub-agent outcomes searchable behind compact previews', () => {
    const longOutcome = `prefix ${'hidden '.repeat(40)}exact outcome token`;
    const fragments = buildSubAgentCardFragments({
      type: 'subagent_summary',
      results: [{ agent_id: 'agent-1', task: 'Inspect renderer', outcome: { type: 'success', result: longOutcome } }],
    }, 'spawn-1');
    expect(fragments).toHaveLength(1);
    expect(fragments[0]?.semanticText).toContain('exact outcome token');
    expect(fragments[0]?.revealTarget).toEqual({
      kind: 'subagent-card',
      toolUseId: 'spawn-1',
      agentId: 'agent-1',
      fragmentId: 'subagent-card:agent-1',
    });
  });

  it('indexes visible fallback labels for result-less sub-agent outcomes', () => {
    const fragments = buildSubAgentCardFragments({
      type: 'subagent_summary',
      results: [
        { agent_id: 'timeout', task: 'Slow task', outcome: { type: 'timed_out', partial_result: '' } },
        { agent_id: 'failed', task: 'Broken task', outcome: { type: 'failure', error: '' } },
        { agent_id: 'success', task: 'Done task', outcome: { type: 'success', result: '' } },
      ],
    }, 'spawn-1');
    expect(fragments.map((fragment) => fragment.semanticText)).toEqual([
      'Slow task\nTimed out: sub-agent exceeded its time limit',
      'Broken task\nFailed',
      'Done task\nCompleted successfully',
    ]);
  });
});

describe('browser tool semantic projection parity', () => {
  it('uses raw visible output for generic browser profile actions', () => {
    const projection = buildTerminalToolResultProjection('opaque', 'CPU throttle set to 4x', { rate: 4 }, { toolUseId: 'profile-1' });
    expect(projection.fullText).toBe('CPU throttle set to 4x');
  });

  it('keeps blocked run-scenario fields and error text searchable', () => {
    const projection = buildTerminalToolResultProjection(
      'browser-profile',
      'Scenario blocked before measurement',
      { blocked_step: 2, reason: 'selector missing', error: 'Scenario blocked before measurement' },
      { toolUseId: 'profile-2' },
    );
    expect(projection.fullText).toContain('blocked step: 2');
    expect(projection.fullText).toContain('reason: selector missing');
    expect(projection.fullText).toContain('error: Scenario blocked before measurement');
  });

  it('normalizes console-log entries, empty labels, and visible tallies', () => {
    const logs = buildTerminalToolResultProjection(
      'console-logs',
      JSON.stringify([{ level: 'error', text: 'boom' }, { level: 'log', text: 'ready' }]),
      undefined,
      { toolUseId: 'logs-1' },
    );
    expect(logs.fullText).toBe('2 entries\n1 error\n1 log\nerror\nboom\nlog\nready');
    expect(buildTerminalToolResultProjection('console-logs', '[]', undefined).fullText).toBe('(no console entries)');
  });
});

describe('buildPatchOutputProjection', () => {
  it('keeps semantic identity stable while carrying the current canonical diff as display text', () => {
    const before = buildPatchOutputProjection('--- a/x\n+++ b/x\n-old\n+new', { toolUseId: 'patch-1' });
    const after = buildPatchOutputProjection('--- a/x\n+++ b/x\n-context\n-old\n+new', { toolUseId: 'patch-1' });
    expect(before.fragments[0].fragmentId).toBe('patch-diff');
    expect(after.fragments[0].fragmentId).toBe(before.fragments[0].fragmentId);
    expect(after.fragments[0].revealTarget).toEqual({
      kind: 'tool-result-patch',
      toolUseId: 'patch-1',
      fragmentId: 'patch-diff',
    });
    expect(after.fullText).toContain('+new');
  });
});

describe('buildAgentTextFragments', () => {
  it('preserves semantic text for compact-collapsed assistant fragments', () => {
    const blocks = [{ type: 'text', text: 'first line\nhidden second line alpha' }] as const;
    const fragments = buildAgentTextFragments(blocks, 'compact');
    expect(fragments).toHaveLength(1);
    expect(fragments[0]?.fragmentId).toBe('agent-text-0');
    expect(fragments[0]?.semanticText).toBe('first line\nhidden second line alpha');
    expect(fragments[0]?.display.mode).toBe('compact-collapsed');
    expect(fragments[0]?.display.summaryText).toBe('first line');
  });

  it('force-expands latest assistant fragments without changing semantic text', () => {
    const blocks = [{ type: 'text', text: 'first line\nhidden second line alpha' }] as const;
    const fragments = buildAgentTextFragments(blocks, 'compact', { forceExpandedText: true });
    expect(fragments[0]?.display.mode).toBe('full');
    expect(fragments[0]?.semanticText).toBe('first line\nhidden second line alpha');
  });
});

describe('buildConversationSearchProjection', () => {
  it('keeps visible compact mermaid text searchable and excludes compact-hidden non-think tool details', () => {
    const units: RenderUnit[] = [
      {
        kind: 'agent_turn',
        key: 'a1',
        isFirstInTurn: true,
        agent: agentMsg('a1', [
          { type: 'text', text: '```mermaid\nflowchart TD\nNode A --> Node B\n```' },
          { type: 'tool_use', id: 'tool-1', name: 'bash', display: 'bash ls', input: { script: 'secret alpha payload' } },
          { type: 'tool_use', id: 'tool-2', name: 'think', display: 'think aloud', input: { thought: 'visible alpha thought' } },
        ]),
        toolResultsByUseId: new Map([
          ['tool-1', toolMsg('t1', 'tool-1', { result: 'hidden alpha result' })],
          ['tool-2', toolMsg('t2', 'tool-2', { result: 'visible alpha result' })],
        ]),
      },
    ];

    const mermaidProjection = buildConversationSearchProjection(units, 'Node B', { density: 'compact' });
    expect(mermaidProjection.matches).toHaveLength(1);
    expect(mermaidProjection.matches[0]?.target.sourceId).toContain('agent-text-0');

    const hiddenToolProjection = buildConversationSearchProjection(units, 'secret alpha payload', { density: 'compact' });
    expect(hiddenToolProjection.matches).toHaveLength(0);

    const thinkProjection = buildConversationSearchProjection(units, 'visible alpha result', { density: 'compact' });
    expect(thinkProjection.matches).toHaveLength(1);
    expect(thinkProjection.matches[0]?.target.sourceId).toContain('tool-use-result-2');
  });

  it('searches complete keyword-search hits in compact display mode', () => {
    const units: RenderUnit[] = [{
      kind: 'agent_turn',
      key: 'keyword-compact',
      isFirstInTurn: true,
      agent: agentMsg('keyword-compact', [
        { type: 'tool_use', id: 'keyword-1', name: 'keyword_search', display: 'search docs', input: { query: 'needle' } },
      ]),
      toolResultsByUseId: new Map([
        ['keyword-1', toolMsg('keyword-result', 'keyword-1', { result: 'src/hidden.ts: compact hidden needle explanation' })],
      ]),
    }];

    const projection = buildConversationSearchProjection(units, 'hidden needle', { density: 'compact' });
    expect(projection.matches).toHaveLength(1);
    const target = projection.matches[0]?.target;
    expect(target?.kind).toBe('unit-text');
    expect(target?.kind === 'unit-text' ? target.fragmentId : undefined).toContain('keyword-search-hit:');
  });

  it('searches complete search-tool hits in compact display mode with typed reveal metadata', () => {
    const units: RenderUnit[] = [{
      kind: 'agent_turn',
      key: 'search-compact',
      isFirstInTurn: true,
      agent: agentMsg('search-compact', [
        { type: 'tool_use', id: 'search-1', name: 'search', display: 'search docs', input: { pattern: 'needle' } },
      ]),
      toolResultsByUseId: new Map([
        ['search-1', toolMsg('search-result', 'search-1', { result: 'src/hidden.ts:42: compact hidden needle explanation' })],
      ]),
    }];

    const projection = buildConversationSearchProjection(units, 'hidden needle', { density: 'compact' });
    expect(projection.matches).toHaveLength(1);
    const target = projection.matches[0]?.target;
    expect(target?.kind).toBe('unit-text');
    expect(target?.kind === 'unit-text' ? target.fragmentId : undefined).toContain('search-hit:');
    expect(projection.sources.some((entry) => entry.fragmentId?.includes('search-hit:'))).toBe(true);
    const matchSource = projection.sources.find((entry) => entry.fragmentId === (target?.kind === 'unit-text' ? target.fragmentId : undefined));
    expect(matchSource?.revealTarget).toMatchObject({ kind: 'tool-result-search', key: 'search:search-1', path: 'src/hidden.ts', lineNumber: 42 });
  });

  it('indexes full-density non-think tool details while compact still excludes them', () => {
    const units: RenderUnit[] = [
      {
        kind: 'agent_turn',
        key: 'a1',
        isFirstInTurn: true,
        agent: agentMsg('a1', [
          { type: 'tool_use', id: 'tool-1', name: 'bash', display: 'bash ls', input: { script: 'secret alpha payload' } },
        ]),
        toolResultsByUseId: new Map([
          ['tool-1', toolMsg('t1', 'tool-1', { result: 'hidden alpha result' })],
        ]),
      },
    ];

    const fullInputProjection = buildConversationSearchProjection(units, 'secret alpha payload', { density: 'full' });
    expect(fullInputProjection.matches).toHaveLength(1);
    expect(fullInputProjection.matches[0]?.target.sourceId).toContain('tool-use-input-0');

    const fullResultProjection = buildConversationSearchProjection(units, 'hidden alpha result', { density: 'full' });
    expect(fullResultProjection.matches).toHaveLength(1);
    expect(fullResultProjection.matches[0]?.target.sourceId).toContain('tool-use-result-0');

    const compactProjection = buildConversationSearchProjection(units, 'secret alpha payload', { density: 'compact' });
    expect(compactProjection.matches).toHaveLength(0);
  });

  it('keeps force-expanded latest compact text searchable past the first line', () => {
    const message = agentMsg('a1', [{ type: 'text', text: 'first line\nvisible second line alpha' }]);
    message.display_data = { forceExpandedText: true };
    const units: RenderUnit[] = [{
      kind: 'agent_turn',
      key: 'a1',
      isFirstInTurn: true,
      agent: message,
      toolResultsByUseId: new Map(),
    }];

    const projection = buildConversationSearchProjection(units, 'visible second line alpha', { density: 'compact' });
    expect(projection.matches).toHaveLength(1);
    expect(projection.matches[0]?.target.sourceId).toContain('agent-text-0');
  });

  it('includes full read_file logical content in compact mode while keeping stable typed fragments', () => {
    const units: RenderUnit[] = [{
      kind: 'agent_turn',
      key: 'a1',
      isFirstInTurn: true,
      agent: agentMsg('a1', [
        { type: 'tool_use', id: 'tool-read', name: 'read_file', display: 'read alpha', input: { path: 'src/foo.ts', offset: 7, limit: 2 } },
      ]),
      toolResultsByUseId: new Map([['tool-read', toolMsg('t1', 'tool-read', { result: '     7\tconst alpha = 1;\n     8\tsecond alpha line' })]]),
    }];

    const compactProjection = buildConversationSearchProjection(units, 'alpha', { density: 'compact' });
    expect(compactProjection.matches).toHaveLength(3);
    const lineMatch = compactProjection.matches.find((match) => match.target.kind === 'unit-text'
      && match.target.fragmentId === 'read-file-line:second%20alpha%20line:0');
    expect(lineMatch).toBeTruthy();
    expect(compactProjection.sources.find((source) => source.fragmentId === 'read-file-path')?.revealTarget).toMatchObject({
      kind: 'tool-result-read-file',
      toolUseId: 'tool-read',
    });
  });

  it('preserves rendered block order across interleaved text and tools', () => {
    const units: RenderUnit[] = [{
      kind: 'agent_turn',
      key: 'interleaved',
      isFirstInTurn: true,
      agent: agentMsg('interleaved', [
        { type: 'text', text: 'first shared token' },
        { type: 'tool_use', id: 'middle-tool', name: 'bash', display: 'middle shared token', input: { cmd: 'echo shared' } },
        { type: 'text', text: 'last shared token' },
      ]),
      toolResultsByUseId: new Map([[
        'middle-tool',
        toolMsg('middle-result', 'middle-tool', { content: JSON.stringify({ status: 'exited', stdout: 'tool shared token', exit_code: 0 }) }),
      ]]),
    }];

    const projection = buildConversationSearchProjection(units, 'shared token', { density: 'full' });
    const matchingSources = projection.matches.map((match) =>
      projection.sources.find((source) => source.id === match.target.sourceId)?.role);
    expect(matchingSources).toEqual([
      'agent-text-0',
      'tool-use-display-1',
      'tool-use-result-1:terminal-result:bash',
      'agent-text-2',
    ]);
  });

  it('uses browser-profile reveal family for every structured profile error', () => {
    const units: RenderUnit[] = [{
      kind: 'agent_turn',
      key: 'metrics-error',
      isFirstInTurn: true,
      agent: agentMsg('metrics-error', [
        { type: 'tool_use', id: 'profile-metrics', name: 'browser_profile', input: { action: 'metrics' } },
      ]),
      toolResultsByUseId: new Map([[
        'profile-metrics',
        {
          ...toolMsg('metrics-result', 'profile-metrics', { error: 'metrics unavailable', is_error: true }),
          display_data: { error: 'metrics unavailable' },
        },
      ]]),
    }];

    const projection = buildConversationSearchProjection(units, 'metrics unavailable', { density: 'compact' });
    const source = projection.sources.find((candidate) => candidate.id === projection.matches[0]?.target.sourceId);
    expect(source?.fragmentId).toBe('terminal-result:browser-profile');
    expect(source?.revealTarget).toMatchObject({
      kind: 'tool-result-terminal',
      family: 'browser-profile',
      toolUseId: 'profile-metrics',
    });
  });

  it('does not synthesize result fragments before a tool result exists', () => {
    const units: RenderUnit[] = [{
      kind: 'agent_turn',
      key: 'pending-tool',
      isFirstInTurn: true,
      agent: agentMsg('pending-tool', [
        { type: 'tool_use', id: 'pending-read', name: 'read_file', display: 'read pending', input: { path: 'pending.ts' } },
      ]),
      toolResultsByUseId: new Map(),
    }];

    expect(buildConversationSearchProjection(units, 'pending.ts', { density: 'compact' }).matches).toHaveLength(0);
    expect(buildConversationSearchProjection(units, 'No matches found.', { density: 'compact' }).matches).toHaveLength(0);
  });

  it('keeps failed read_file output as an opaque error fragment', () => {
    const units: RenderUnit[] = [{
      kind: 'agent_turn',
      key: 'failed-read',
      isFirstInTurn: true,
      agent: agentMsg('failed-read', [
        { type: 'tool_use', id: 'failed-read-tool', name: 'read_file', display: 'read missing', input: { path: 'missing.ts' } },
      ]),
      toolResultsByUseId: new Map([[
        'failed-read-tool',
        toolMsg('failed-result', 'failed-read-tool', { error: 'permission denied', is_error: true }),
      ]]),
    }];

    const projection = buildConversationSearchProjection(units, 'permission denied', { density: 'compact' });
    expect(projection.matches).toHaveLength(1);
    const source = projection.sources.find((candidate) => candidate.id === projection.matches[0]?.target.sourceId);
    expect(source?.fragmentId).toBe('terminal-result:opaque');
    expect(source?.revealTarget).toMatchObject({
      kind: 'tool-result-terminal',
      toolUseId: 'failed-read-tool',
      family: 'opaque',
    });
    expect(projection.sources.some((candidate) => candidate.fragmentId === 'read-file-path')).toBe(false);
  });

  it('includes expanded system prompt text in transcript projection', () => {
    const projection = buildConversationSearchProjection([], 'alpha directive', {
      systemPrompt: 'alpha directive\nsecondary line',
      systemPromptExpanded: true,
    });

    expect(projection.matches).toHaveLength(1);
    expect(projection.sources[0]?.role).toBe('system-prompt');
    expect(projection.sources[0]?.text).toBe('alpha directive\nsecondary line');
    expect(projection.matches[0]?.target.kind).toBe('header-text');
  });

  it('keeps skill transcript matches to visible inline trigger and file chips', () => {
    const units: RenderUnit[] = [{
      kind: 'skill',
      key: 's1',
      message: {
        ...skillMsg('s1', 'Skill alpha'),
        content: {
          text: 'Skill alpha',
          name: 'dogfood',
          trigger: '/dogfood alpha --trace',
          args: 'alpha --trace',
          source: '/skills/dogfood/SKILL.md',
          snippet: 'Alpha walkthrough',
          files: [{ original_name: 'alpha.txt', media_type: 'text/plain', size_bytes: 5, stored_path: '/tmp/alpha.txt' }],
        } as unknown as Message['content'],
      },
    }];

    expect(buildConversationSearchProjection(units, '/dogfood alpha --trace').matches).toHaveLength(1);
    expect(buildConversationSearchProjection(units, 'alpha.txt').matches).toHaveLength(1);
    expect(buildConversationSearchProjection(units, '/skills/dogfood/SKILL.md').matches).toHaveLength(0);
    expect(buildConversationSearchProjection(units, 'Alpha walkthrough').matches).toHaveLength(0);
  });
  it('includes legacy plain-string user message content in conversation search', () => {
    const units: RenderUnit[] = [{
      kind: 'user',
      key: 'u1',
      message: {
        message_id: 'u1',
        sequence_id: 1,
        conversation_id: 'c1',
        message_type: 'user',
        content: 'Legacy alpha body',
        created_at: '',
      } as unknown as Message,
    }];

    const projection = buildConversationSearchProjection(units, 'alpha');
    expect(projection.sources.map((source) => source.text)).toEqual(['Legacy alpha body']);
    expect(projection.matches).toHaveLength(1);
  });

  it('projects canonical typed content across available render units exhaustively', () => {
    const awaiting: Extract<ConversationState, { type: 'awaiting_sub_agents' }> = {
      type: 'awaiting_sub_agents',
      pending: [{ agent_id: 'sa1', task: 'inspect alpha path' }],
      completed_results: [{ agent_id: 'sa2', task: 'summarize beta path', outcome: { type: 'success', result: 'done' } }],
    };
    const units: RenderUnit[] = [
      { kind: 'user', key: 'u1', message: userMsg('u1', 'User alpha', [{ original_name: 'alpha.txt' }]) },
      { kind: 'pending_user', key: 'p1', message: queued('p1', 'Pending alpha', [{ original_name: 'queued-alpha.md', media_type: 'text/markdown', size_bytes: 12, stored_path: '/tmp/queued-alpha.md' }]) },
      {
        kind: 'skill',
        key: 's1',
        message: {
          ...skillMsg('s1', 'Skill alpha'),
          content: {
            text: 'Skill alpha',
            name: 'dogfood',
            trigger: '/dogfood alpha --trace',
            args: 'alpha --trace',
            source: '/skills/dogfood/SKILL.md',
            snippet: 'Alpha walkthrough',
            files: [{ original_name: 'alpha.txt', media_type: 'text/plain', size_bytes: 5, stored_path: '/tmp/alpha.txt' }],
          } as unknown as Message['content'],
        },
      },
      { kind: 'system', key: 'sys1', message: systemMsg('sys1', 'System alpha') },
      {
        kind: 'agent_turn',
        key: 'a1',
        isFirstInTurn: true,
        agent: agentMsg('a1', [
          { type: 'text', text: 'Agent alpha' },
          { type: 'tool_use', id: 'tool-1', name: 'search', display: 'search alpha', input: { pattern: 'alpha', path: 'src' } },
        ]),
        toolResultsByUseId: new Map([['tool-1', toolMsg('t1', 'tool-1', { result: 'Tool alpha result' })]]),
      },
      { kind: 'sub_agent_status', key: 'sub', state: awaiting },
      { kind: 'streaming_agent', key: 'stream', isFirstInTurn: false },
    ];

    const projection = buildConversationSearchProjection(units, 'alpha');
    expect(projection.sources.map((source) => [source.unitKind, source.role, source.text])).toEqual([
      ['user', 'user-message', 'User alpha\nalpha.txt'],
      ['pending_user', 'pending-user-message', 'Pending alpha\nqueued-alpha.md'],
      ['skill', 'skill-message', '/dogfood alpha --trace\nalpha.txt'],
      ['system', 'system-message', 'System alpha'],
      ['agent_turn', 'agent-text-0', 'Agent alpha'],
      ['agent_turn', 'tool-use-name-1', 'search'],
      ['agent_turn', 'tool-use-display-1', 'search alpha'],
      ['agent_turn', 'tool-use-input-1', '{\n  "path": "src",\n  "pattern": "alpha"\n}'],
      ['agent_turn', 'tool-use-result-1:search-note:Tool%20alpha%20result:0', 'Tool alpha result'],
      ['sub_agent_status', 'sub-agent-status', 'completed summarize beta path done\npending inspect alpha path'],
    ]);
    const unitTargets = projection.matches
      .map((match) => match.target)
      .filter((target) => target.kind === 'unit-text');
    expect(unitTargets.map((target) => target.unitKind)).toEqual([
      'user',
      'user',
      'pending_user',
      'pending_user',
      'skill',
      'skill',
      'system',
      'agent_turn',
      'agent_turn',
      'agent_turn',
      'agent_turn',
      'sub_agent_status',
    ]);
    expect(unitTargets.every((target) => target.unitKind !== 'streaming_agent')).toBe(true);
  });
});
