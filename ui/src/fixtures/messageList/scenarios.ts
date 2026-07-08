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
    content: { text: 'Run the quick checks and summarize the result.' },
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
      { type: 'tool_use', id: 'tool-bash', name: 'bash', input: { op: 'run', cmd: 'pnpm test -- MessageList.test.tsx' }, display: 'pnpm test -- MessageList.test.tsx' },
      { type: 'tool_use', id: 'tool-patch', name: 'patch', input: { path: 'ui/src/components/MessageList.tsx' } },
      { type: 'text', text: 'The tool details stay compact, while prose remains readable enough for scanning.' },
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
    content: { tool_use_id: 'tool-bash', content: '2 tests passed', is_error: false },
    display_data: {},
  },
  {
    message_id: 'tool-result-patch',
    conversation_id: 'fixture-message-list',
    sequence_id: 4,
    type: 'tool',
    message_type: 'tool',
    created_at: '2025-01-01T10:02:00.000Z',
    content: { tool_use_id: 'tool-patch', content: 'Applied patch', is_error: false },
    display_data: {},
  },
  {
    message_id: 'user-tool-2',
    conversation_id: 'fixture-message-list',
    sequence_id: 5,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T10:03:00.000Z',
    content: { text: 'Great. What changed?' },
    display_data: {},
  },
  {
    message_id: 'agent-tool-2',
    conversation_id: 'fixture-message-list',
    sequence_id: 6,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:04:00.000Z',
    content: [{ type: 'text', text: 'The fixture now shows the compact transcript in a real scroll container.\n\nThe latest assistant summary remains expanded so the end state is visible without an extra click.' }],
    display_data: {},
  },
];

const markdownImageMessages: Message[] = [
  {
    message_id: 'user-image-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 1,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T10:00:00.000Z',
    content: { text: 'Please include the screenshot preview in your summary.' },
    display_data: {},
  },
  {
    message_id: 'agent-image-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 2,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:01:00.000Z',
    content: [{
      type: 'text',
      text: [
        'Here is the Markdown screenshot preview using the same syntax agents paste into conversations:',
        '',
        '![file-tree-dark-single-slot](qa/message-list/markdown-image-fixture.svg)',
        '',
        'The image is constrained to the message column and keeps its aspect ratio.',
      ].join('\n'),
    }],
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
    title: 'Compact tool strip',
    description: 'Compact density still collapses tool detail into the inline pill strip.',
    theme: 'dark',
  },
  {
    id: 'markdown-image-dark',
    title: 'Markdown image',
    description: 'Assistant Markdown image syntax renders an inline screenshot preview.',
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
  const messages = scenario.id === 'compact-tool-strip'
    ? toolStripMessages
    : scenario.id === 'markdown-image-dark'
      ? markdownImageMessages
      : baseMessages;
  return {
    conversationId: `fixture-message-list-${scenario.id}`,
    slug: `fixture-message-list-${scenario.id}`,
    theme: scenario.theme,
    messages,
    pendingMessages: [],
    convState: { type: 'idle' },
  };
}
