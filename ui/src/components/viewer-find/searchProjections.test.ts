import { describe, expect, it } from 'vitest';
import type { ContentBlock, ConversationState, Message } from '../../api';
import type { QueuedMessage } from '../../hooks/useMessageQueue';
import type { RenderUnit } from '../../conversation/renderUnits';
import {
  buildConversationSearchProjection,
  buildDiffSearchProjection,
  buildFileSearchProjection,
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
      { kind: 'file-line', lineNumber: 1, startColumn: 0, endColumn: 5 },
      { kind: 'file-line', lineNumber: 2, startColumn: 5, endColumn: 10 },
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
      ['skill', 'skill-message', '/dogfood alpha --trace\n/skills/dogfood/SKILL.md\nAlpha walkthrough\nalpha.txt'],
      ['system', 'system-message', 'System alpha'],
      ['agent_turn', 'agent-text-0', 'Agent alpha'],
      ['agent_turn', 'tool-use-name-1', 'search'],
      ['agent_turn', 'tool-use-display-1', 'search alpha'],
      ['agent_turn', 'tool-use-input-1', '{\n  "path": "src",\n  "pattern": "alpha"\n}'],
      ['agent_turn', 'tool-use-result-1', 'Tool alpha result'],
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
