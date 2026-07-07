import type { Message } from '../../api';
import type { MessageListFixtureData, MessageListScenario } from './types';

const baseMessages: Message[] = [
  {
    message_id: 'user-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 1,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T10:00:00.000Z',
    content: { text: 'Please summarize the plan.' },
    display_data: {},
  },
  {
    message_id: 'agent-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 2,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:01:00.000Z',
    content: [{ type: 'text', text: 'I checked the code and the first pass is complete.\n\nThe result is ready to review.' }],
    display_data: {},
  },
  {
    message_id: 'user-2',
    conversation_id: 'fixture-message-list',
    sequence_id: 3,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T10:02:00.000Z',
    content: { text: 'Anything else?' },
    display_data: {},
  },
  {
    message_id: 'agent-2',
    conversation_id: 'fixture-message-list',
    sequence_id: 4,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:03:00.000Z',
    content: [{ type: 'text', text: 'Done — I shipped the update and the final summary is here.\n\nYou can proceed with the next step.' }],
    display_data: {},
  },
];
const toolStripMessages: Message[] = [
  {
    message_id: 'user-tool-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 1,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T10:00:00.000Z',
    content: { text: 'Find where compact tool rendering is implemented and inspect the relevant files.' },
    display_data: {},
  },
  {
    message_id: 'agent-tool-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 2,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:01:00.000Z',
    content: [
      { type: 'text', text: 'I am checking the targeted surface first.' },
      { type: 'tool_use', id: 'tool-think', name: 'think', input: { thoughts: 'Verify compact density without expanding every historical tool detail.' } },
      { type: 'tool_use', id: 'tool-search-1', name: 'search', input: { pattern: 'CompactToolStrip|deriveToolStripItems', path: 'ui/src', include: '*.tsx' } },
      { type: 'tool_use', id: 'tool-read-1', name: 'read_file', input: { path: 'ui/src/components/MessageComponents.tsx', offset: 811, limit: 80 } },
      { type: 'tool_use', id: 'tool-search-2', name: 'search', input: { pattern: 'compact-tool', path: 'ui/src', include: '*.css' } },
      { type: 'tool_use', id: 'tool-read-2', name: 'read_file', input: { path: 'ui/src/components/agentTurnToolStrip.ts', offset: 1, limit: 120 } },
      { type: 'tool_use', id: 'tool-bash', name: 'bash', input: { op: 'run', cmd: 'pnpm vitest run src/components/agentTurnToolStrip.test.ts' }, display: 'pnpm vitest run src/components/agentTurnToolStrip.test.ts' },
      { type: 'tool_use', id: 'tool-patch', name: 'patch', input: { path: 'ui/src/components/MessageComponents.tsx', patches: [{ operation: 'replace' }, { operation: 'insert_after' }] } },
      { type: 'text', text: 'The compact cards show what each repeated tool did without expanding the full details.' },
    ],
    display_data: {},
  },
  {
    message_id: 'tool-result-bash',
    conversation_id: 'fixture-message-list',
    sequence_id: 3,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:01:30.000Z',
    content: { tool_use_id: 'tool-search-1', content: 'ui/src/components/MessageComponents.tsx:822:function CompactToolStripImpl\nui/src/components/agentTurnToolStrip.ts:32:export function deriveToolStripItems', is_error: false },
    display_data: {},
  },
  {
    message_id: 'tool-result-read-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 4,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:01:40.000Z',
    content: { tool_use_id: 'tool-read-1', content: Array.from({ length: 80 }, (_, i) => `${i + 811}\tcompact tool rendering line`).join('\n'), is_error: false },
    display_data: {},
  },
  {
    message_id: 'tool-result-search-2',
    conversation_id: 'fixture-message-list',
    sequence_id: 5,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:01:50.000Z',
    content: { tool_use_id: 'tool-search-2', content: 'ui/src/index.css:321:.compact-tool-strip {\nui/src/index.css:334:.compact-tool-card {\nui/src/index.css:387:.compact-tool-card-summary {', is_error: false },
    display_data: {},
  },
  {
    message_id: 'tool-result-read-2',
    conversation_id: 'fixture-message-list',
    sequence_id: 6,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:02:00.000Z',
    content: { tool_use_id: 'tool-read-2', content: Array.from({ length: 120 }, (_, i) => `${i + 1}\texport const compactSummaryFixture = true;`).join('\n'), is_error: false },
    display_data: {},
  },
  {
    message_id: 'tool-result-bash',
    conversation_id: 'fixture-message-list',
    sequence_id: 7,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:02:10.000Z',
    content: { tool_use_id: 'tool-bash', content: JSON.stringify({ status: 'exited', exit_code: 0, lines: [] }), is_error: false },
    display_data: {},
  },
  {
    message_id: 'tool-result-patch',
    conversation_id: 'fixture-message-list',
    sequence_id: 8,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:02:20.000Z',
    content: { tool_use_id: 'tool-patch', content: 'Applied patch', is_error: false },
    display_data: {},
  },
  {
    message_id: 'user-tool-2',
    conversation_id: 'fixture-message-list',
    sequence_id: 9,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T10:03:00.000Z',
    content: { text: 'Great. What changed?' },
    display_data: {},
  },
  {
    message_id: 'agent-tool-2',
    conversation_id: 'fixture-message-list',
    sequence_id: 10,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:04:00.000Z',
    content: [{ type: 'text', text: 'The fixture now shows the compact transcript in a real scroll container.\n\nThe latest assistant summary remains expanded so the end state is visible without an extra click.' }],
    display_data: {},
  },
];


export const messageListScenarios = [
  {
    id: 'compact-latest-expanded',
    title: 'Compact latest expanded',
    description: 'Latest finalized assistant summary stays expanded in compact density.',
    theme: 'dark',
  },
  {
    id: 'compact-tool-strip',
    title: 'Compact tool summaries',
    description: 'Compact density collapses repeated tool detail into scannable summary cards.',
    theme: 'dark',
  },
] as const satisfies readonly MessageListScenario[];

export type MessageListScenarioId = (typeof messageListScenarios)[number]['id'];

export function getMessageListScenario(id: MessageListScenarioId): MessageListScenario {
  const scenario = messageListScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown message list scenario: ${id}`);
  return scenario;
}

export function messageListFixtureData(scenario: MessageListScenario): MessageListFixtureData {
  const messages = scenario.id === 'compact-tool-strip' ? toolStripMessages : baseMessages;
  return {
    conversationId: `fixture-message-list-${scenario.id}`,
    slug: `fixture-message-list-${scenario.id}`,
    theme: scenario.theme,
    messages,
    pendingMessages: [],
    convState: { type: 'idle' },
  };
}
