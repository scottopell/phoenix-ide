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

const scrollPolicyMessages: Message[] = Array.from({ length: 80 }, (_, index) => {
  const sequenceId = index + 1;
  const isUser = index % 2 === 0;
  return {
    message_id: `scroll-policy-${sequenceId}`,
    conversation_id: 'fixture-message-list-scroll-policy',
    sequence_id: sequenceId,
    type: isUser ? 'user' : 'agent',
    message_type: isUser ? 'user' : 'agent',
    created_at: new Date(Date.UTC(2025, 0, 1, 10, index)).toISOString(),
    content: isUser
      ? { text: `Scroll policy checkpoint ${sequenceId}: keep this historical item stable.` }
      : [{
          type: 'text',
          text: `Checkpoint ${sequenceId} is complete.\n\n${'Measured conversation output remains deterministic. '.repeat(6)}`,
        }],
    display_data: {},
  } as Message;
});

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
        '![file-tree-dark-single-slot](http://127.0.0.1:61123/qa/message-list/markdown-image-fixture.svg)',
        '',
        'The image is constrained to the message column and keeps its aspect ratio.',
      ].join('\n'),
    }],
    display_data: {},
  },
];

const wideMarkdownTableMessages: Message[] = [
  {
    message_id: 'user-wide-table-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 1,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T10:00:00.000Z',
    content: { text: 'Compare the operating models in a table.' },
    display_data: {},
  },
  {
    message_id: 'agent-wide-table-1',
    conversation_id: 'fixture-message-list',
    sequence_id: 2,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T10:01:00.000Z',
    content: [{
      type: 'text',
      text: [
        'The prose remains in the readable conversation column while the comparison uses the available pane width.',
        '',
        '| Operating model | Test world | Scarce expertise | Operational coupling | Primary consumers | Success horizon |',
        '| --- | --- | --- | --- | --- | --- |',
        '| Shared platform | kernels, fuzzing, workload replay | Linux, eBPF, security | release and fleet teams | infrastructure product groups | performance and correctness |',
        '| Specialist program | device fixtures and adversarial testing | GPU and runtime integration | artifact owners | feature delivery teams | durable expertise transfer |',
        '| Temporary initiative | large-cluster chaos and rollout tests | Kubernetes controllers | onboarding teams | service owners | convergence and safe migration |',
        '',
        'The paragraph after the table returns to the same readable prose width.',
      ].join('\n'),
    }],
    display_data: {},
  },
];


const continuityParagraphs = Array.from({ length: 28 }, (_, index) => (
  `Continuity marker ${String(index + 1).padStart(2, '0')}: this is deterministic tall-row text used to keep a precise reading position visible while earlier history is inserted.`
));

const prefixContinuityMessages: Message[] = [
  {
    message_id: 'continuity-user-anchor',
    conversation_id: 'fixture-message-list-prefix-continuity',
    sequence_id: 101,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T11:00:00.000Z',
    content: { text: 'Give me a detailed walkthrough with enough depth to read midway through it.' },
    display_data: {},
  },
  {
    message_id: 'continuity-agent-anchor',
    conversation_id: 'fixture-message-list-prefix-continuity',
    sequence_id: 102,
    type: 'agent',
    message_type: 'agent',
    created_at: '2025-01-01T11:01:00.000Z',
    content: [{ type: 'text', text: continuityParagraphs.join('\n\n') }],
    display_data: {},
  },
  {
    message_id: 'continuity-user-tail',
    conversation_id: 'fixture-message-list-prefix-continuity',
    sequence_id: 103,
    type: 'user',
    message_type: 'user',
    created_at: '2025-01-01T11:02:00.000Z',
    content: { text: 'This tail message keeps the tall response away from the list boundary.' },
    display_data: {},
  },
];

export const prefixContinuityEarlierMessages: Message[] = Array.from({ length: 18 }, (_, index) => ({
  message_id: `continuity-prefix-${index + 1}`,
  conversation_id: 'fixture-message-list-prefix-continuity',
  sequence_id: index + 1,
  type: index % 2 === 0 ? 'user' : 'agent',
  message_type: index % 2 === 0 ? 'user' : 'agent',
  created_at: `2025-01-01T10:${String(index).padStart(2, '0')}:00.000Z`,
  content: index % 2 === 0
    ? { text: `Earlier user message ${index + 1}` }
    : [{ type: 'text', text: `Earlier assistant response ${index + 1}. `.repeat(8) }],
  display_data: {},
} as Message));

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
  {
    id: 'scroll-policy-long',
    title: 'Scroll policy long conversation',
    description: 'Long deterministic conversation with controls for real VirtualTranscript tail-follow QA.',
    theme: 'dark',
  },
  {
    id: 'prefix-continuity-offset-bug',
    title: 'Prefix continuity offset bug',
    description: 'Interactive real-VirtualTranscript reproduction of identity-only restoration jumping within a tall row.',
    theme: 'dark',
  },
  {
    id: 'wide-markdown-table',
    title: 'Wide Markdown table',
    description: 'Wide assistant tables expand beyond prose while staying inside the conversation pane.',
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
      : scenario.id === 'wide-markdown-table'
        ? wideMarkdownTableMessages
        : scenario.id === 'scroll-policy-long'
          ? scrollPolicyMessages
          : scenario.id === 'prefix-continuity-offset-bug'
            ? prefixContinuityMessages
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
