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
});

describe('buildConversationSearchProjection', () => {
  it('projects canonical typed content across available render units exhaustively', () => {
    const awaiting: Extract<ConversationState, { type: 'awaiting_sub_agents' }> = {
      type: 'awaiting_sub_agents',
      pending: [{ agent_id: 'sa1', task: 'inspect alpha path' }],
      completed_results: [{ agent_id: 'sa2', task: 'summarize beta path', outcome: { type: 'success', result: 'done' } }],
    };
    const units: RenderUnit[] = [
      { kind: 'user', key: 'u1', message: userMsg('u1', 'User alpha', [{ original_name: 'alpha.txt' }]) },
      { kind: 'pending_user', key: 'p1', message: queued('p1', 'Pending alpha', [{ original_name: 'queued-alpha.md', media_type: 'text/markdown', size_bytes: 12, stored_path: '/tmp/queued-alpha.md' }]) },
      { kind: 'skill', key: 's1', message: skillMsg('s1', 'Skill alpha') },
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
      ['skill', 'skill-message', 'Skill alpha'],
      ['system', 'system-message', 'System alpha'],
      ['agent_turn', 'agent-text-0', 'Agent alpha'],
      ['agent_turn', 'tool-use-name-1', 'search'],
      ['agent_turn', 'tool-use-display-1', 'search alpha'],
      ['agent_turn', 'tool-use-input-1', '{\n  "path": "src",\n  "pattern": "alpha"\n}'],
      ['agent_turn', 'tool-use-result-1', 'Tool alpha result'],
      ['sub_agent_status', 'sub-agent-status', 'pending inspect alpha path\ncompleted summarize beta path done'],
    ]);
    expect(projection.matches.map((match) => match.target.unitKind)).toEqual([
      'user',
      'user',
      'pending_user',
      'pending_user',
      'skill',
      'system',
      'agent_turn',
      'agent_turn',
      'agent_turn',
      'agent_turn',
      'sub_agent_status',
    ]);
    expect(projection.matches.every((match) => match.target.unitKind !== 'streaming_agent')).toBe(true);
  });
});
