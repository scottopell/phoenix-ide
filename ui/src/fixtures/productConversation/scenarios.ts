import type { ChainView, ConversationState, Message, ProductConversationSnapshotView } from '../../api';
import type { EnrichedMessage } from '../../generated/EnrichedMessage';
import { productConversationScenarioDefinitions } from './types';
import type { ProductConversationScenario, ProductConversationScenarioId } from './types';

const now = Date.parse('2026-07-01T12:00:00Z');
const isoAgo = (minutes: number) => new Date(now - minutes * 60_000).toISOString();

type FixtureMessage = EnrichedMessage & Message;

const ALIGNED_PREFIX_TRANSCRIPT_ROW_ID = 'row-aligned-prefix-tail';

function state(type: ConversationState['type']): ConversationState {
  switch (type) {
    case 'idle':
    case 'awaiting_llm':
    case 'terminal':
      return { type };
    case 'error':
      return { type, message: 'Fixture conversation error', error_kind: 'server_error' };
    default:
      return { type } as ConversationState;
  }
}

function textMessage(id: string, sequenceId: number, messageType: 'user' | 'agent' | 'system', text: string, conversationState?: ConversationState): FixtureMessage {
  return {
    message_id: id,
    conversation_id: ALIGNED_PREFIX_TRANSCRIPT_ROW_ID,
    sequence_id: sequenceId,
    message_type: messageType,
    content: messageType === 'agent' ? [{ type: 'text' as const, text }] : { text },
    display_data: conversationState ? { conversation_state: conversationState } : null,
    usage_data: null,
    created_at: isoAgo(500 - sequenceId),
  };
}

function segment(
  segmentOrdinal: number,
  transcriptRowId: string,
  title: string,
  messages: EnrichedMessage[],
  handoffSummary: string | null,
) {
  return {
    segment_ordinal: segmentOrdinal,
    transcript_row_id: transcriptRowId,
    slug: transcriptRowId,
    title,
    messages,
    handoff: handoffSummary === null ? null : {
      kind: 'historical' as const,
      predecessor_transcript_row_id: `prev-${transcriptRowId}`,
      successor_transcript_row_id: transcriptRowId,
      continuation_message_id: `continue-${transcriptRowId}`,
      summary: handoffSummary,
    },
  };
}

function makeSnapshot(overrides: Partial<ProductConversationSnapshotView> = {}): ProductConversationSnapshotView {
  return {
    product_conversation_id: 'pc-product-alpha',
    close: null,
    canonical_route: '/product-conversations/pc-product-alpha',
    requested_transcript_row_id: 'row-work',
    canonical_root: { transcript_row_id: 'row-root', slug: 'product-alpha-root', title: 'Product Alpha root' },
    ordinary_lifecycle: 'open',
    latest_transcript_row_id: 'row-work',
    writable_transcript_row_id: null,
    updated_at: isoAgo(2),
    presentation: { kind: 'state', display_name: 'Product Alpha', presentation_mode: 'working' },
    work_identity: {
      work_transcript_row_id: 'row-work',
      worktree_path: '/Users/scottopell/dev/phoenix-ide/.phoenix/worktrees/product-alpha',
      branch_name: 'task-40012-retire-chain-product-surface',
      base_branch: 'main',
      task_id: '40012',
      task_title: 'Retire chain product surface',
    },
    source: {
      status: 'present',
      source_product_conversation_id: 'pc-source',
      source_conversation_id: 'conv-source',
      relation: 'approved_task',
      relation_key: 'task-40012',
    },
    chain_qa_compatibility: { root_transcript_row_id: 'chain-root-product-alpha', url: '/chains/chain-root-product-alpha' },
    segments: [
      segment(1, 'row-root', 'Discovery', [
        textMessage('root-1', 1, 'user', 'Summarize the product-surface issue.'),
        textMessage('root-2', 2, 'agent', 'The chain route and product route overlap in a way that confuses ownership.'),
      ], 'Exploration converged on product-conversation routing.'),
      segment(2, 'row-qa', 'Question answering', [
        textMessage('qa-1', 3, 'user', 'What are the invariants we must preserve?'),
        textMessage('qa-2', 4, 'agent', 'We must preserve the transcript ordering, source lineage, and Q&A history.'),
      ], 'The focused Q&A fork captured the invariants before work began.'),
      segment(3, 'row-work', 'Implementation', [
        textMessage('work-1', 5, 'user', 'Proceed with the frontend-only fixture implementation.'),
        textMessage('work-2', 6, 'agent', 'I will add a deterministic ProductConversation Ladle fixture and keep the history read-only.', state('idle')),
      ], null),
    ],
    before: null,
    has_older: false,
    ...overrides,
  };
}

export const ALIGNED_PREFIX_BOUNDARY_TOOL_ID = 'aligned-prefix-boundary-tool';
export const ALIGNED_PREFIX_STEERING_TOOL_ID = 'aligned-prefix-steering-tool';
export const ALIGNED_PREFIX_TERMINAL_MARKER = 'TRANSCRIPT_TAIL_TERMINAL_ASSISTANT_VISIBLE';

function toolUseMessage(id: string, sequenceId: number, toolUseId: string, name: string, input: Record<string, unknown>): FixtureMessage {
  return {
    ...textMessage(id, sequenceId, 'agent', ''),
    content: [{ type: 'tool_use' as const, id: toolUseId, name, input }],
    display_data: {},
  };
}

function toolResultMessage(id: string, sequenceId: number, toolUseId: string, content: string): FixtureMessage {
  return {
    ...textMessage(id, sequenceId, 'system', ''),
    message_type: 'tool' as const,
    content: { tool_use_id: toolUseId, content, is_error: false },
    display_data: {},
  };
}

function makeAlignedPrefixScenario() {
  const prefixOwner = toolUseMessage('aligned-prefix-owner', 1, ALIGNED_PREFIX_BOUNDARY_TOOL_ID, 'read_file', { path: 'ui/src/incident-boundary.ts' });
  const snapshotMessages = [
    toolResultMessage('aligned-prefix-result', 2, ALIGNED_PREFIX_BOUNDARY_TOOL_ID, 'boundary owner completed'),
    ...Array.from({ length: 70 }, (_, index) => toolResultMessage(
      `aligned-prefix-window-row-${index + 3}`,
      index + 3,
      `aligned-prefix-window-tool-${index + 3}`,
      `Persisted boundary-window row ${index + 3}`,
    )),
    ...Array.from({ length: 24 }, (_, index) => textMessage(
      `aligned-prefix-history-${index + 73}`,
      index + 73,
      index % 2 === 0 ? 'user' : 'agent',
      `${'Variable-height aggregate transcript evidence. '.repeat((index % 4) + 1)} Sequence ${index + 73}.`,
    )),
    toolUseMessage('aligned-prefix-steering-use', 97, ALIGNED_PREFIX_STEERING_TOOL_ID, 'send_conversation_message', {
      target: '@conv:fixture-target',
      message: 'Continue from the durable checkpoint.',
      message_id: 'fixture-steering-message-id',
    }),
    toolResultMessage(
      'aligned-prefix-steering-result',
      98,
      ALIGNED_PREFIX_STEERING_TOOL_ID,
      JSON.stringify({ outcome: 'queued_as_steering', target: '@conv:fixture-target', message_id: 'fixture-steering-message-id' }),
    ),
    toolUseMessage('aligned-prefix-followup-use', 99, 'aligned-prefix-followup-tool', 'read_file', { path: 'ui/src/final-check.ts' }),
    toolResultMessage('aligned-prefix-followup-result', 100, 'aligned-prefix-followup-tool', 'final check completed'),
    textMessage(
      'aligned-prefix-terminal-assistant',
      101,
      'agent',
      `${ALIGNED_PREFIX_TERMINAL_MARKER}: the durable terminal assistant response is visible.`,
      state('idle'),
    ),
  ];
  const snapshot = makeSnapshot({
    product_conversation_id: 'pc-aligned-prefix-tail',
    canonical_route: '/product-conversations/pc-aligned-prefix-tail',
    requested_transcript_row_id: 'row-aligned-prefix-tail',
    latest_transcript_row_id: 'row-aligned-prefix-tail',
    writable_transcript_row_id: 'row-aligned-prefix-tail',
    canonical_root: { transcript_row_id: 'row-aligned-prefix-tail', slug: 'aligned-prefix-tail', title: 'Aligned prefix tail' },
    presentation: { kind: 'state', display_name: 'Aligned prefix tail', presentation_mode: 'idle' },
    work_identity: null,
    source: null,
    chain_qa_compatibility: null,
    segments: [segment(1, 'row-aligned-prefix-tail', 'Incident-shaped latest row', snapshotMessages, null)],
    before: 'aligned-prefix-older-cursor',
    has_older: true,
  });
  const olderSnapshot = makeSnapshot({
    ...snapshot,
    segments: [segment(1, 'row-aligned-prefix-tail', 'Incident-shaped latest row', [
      textMessage('aligned-prefix-older-reader-anchor', 0, 'user', 'OLDER_READER_ANCHOR_PRESERVED'),
    ], null)],
    before: null,
    has_older: false,
  });
  return { snapshot, olderSnapshot, alignedLatestMessages: [prefixOwner, ...snapshotMessages] };
}

const alignedPrefixScenario = makeAlignedPrefixScenario();

function makeLongSnapshot(): ProductConversationSnapshotView {
  let sequenceId = 1;
  const segments = Array.from({ length: 4 }, (_, segmentIndex) => {
    const messageCount = segmentIndex === 3 ? 26 : 28;
    const messages = Array.from({ length: messageCount }, (_, messageIndex) => {
      const current = sequenceId++;
      const type = current % 2 === 0 ? 'agent' as const : 'user' as const;
      const prefix = segmentIndex === 3 && messageIndex === messageCount - 1
        ? 'Final status summary'
        : `Segment ${segmentIndex + 1} message ${messageIndex + 1}`;
      return textMessage(
        `long-${current}`,
        current,
        type,
        `${prefix}: deterministic fixture transcript content for chronology validation.`,
        current === 110 ? state('idle') : undefined,
      );
    });
    return segment(
      segmentIndex + 1,
      `row-long-${segmentIndex + 1}`,
      `Stage ${segmentIndex + 1}`,
      messages,
      segmentIndex === 0 ? 'Earlier product discussion condensed into a single historical handoff.' : null,
    );
  });

  return makeSnapshot({
    product_conversation_id: 'pc-long-history',
    canonical_route: '/product-conversations/pc-long-history',
    requested_transcript_row_id: 'row-long-4',
    latest_transcript_row_id: 'row-long-4',
    writable_transcript_row_id: 'row-long-4',
    canonical_root: { transcript_row_id: 'row-long-1', slug: 'long-root', title: 'Long history root' },
    presentation: { kind: 'state', display_name: 'Long fixture conversation', presentation_mode: 'idle' },
    work_identity: null,
    source: null,
    chain_qa_compatibility: null,
    segments,
    has_older: true,
    before: 'fixture-older-cursor',
  });
}

function makeChain(rootConvId = 'chain-root-product-alpha'): ChainView {
  return {
    root_conv_id: rootConvId,
    chain_name: null,
    display_name: 'Product Alpha',
    archived: false,
    members: [],
    qa_history: [
      {
        id: 'fixture-recall-completed',
        root_conv_id: rootConvId,
        question: 'Which invariants carried across the whole conversation?',
        answer: 'The lineage kept one chronological transcript, source provenance, and a single ordinary composer on the latest row.',
        model: 'fixture-model',
        status: 'completed',
        chain_members_at_answer: 2,
        chain_messages_at_answer: 4,
        created_at: isoAgo(28),
        completed_at: isoAgo(27),
      },
      {
        id: 'fixture-recall-failed',
        root_conv_id: rootConvId,
        question: 'Did the first visual experiment survive?',
        answer: 'The experiment showed that a full-height diagnostics column',
        model: 'fixture-model',
        status: 'failed',
        chain_members_at_answer: 3,
        chain_messages_at_answer: 6,
        created_at: isoAgo(18),
        completed_at: isoAgo(17),
      },
    ],
    current_member_count: 3,
    current_total_messages: 6,
    work_identity: null,
  };
}

function makeOlderPage(): ProductConversationSnapshotView {
  return makeSnapshot({
    product_conversation_id: 'pc-long-history',
    canonical_route: '/product-conversations/pc-long-history',
    requested_transcript_row_id: 'row-long-4',
    latest_transcript_row_id: 'row-long-4',
    writable_transcript_row_id: 'row-long-4',
    canonical_root: { transcript_row_id: 'row-long-0', slug: 'long-older-root', title: 'Long older root' },
    presentation: { kind: 'state', display_name: 'Long fixture conversation', presentation_mode: 'idle' },
    work_identity: null,
    source: null,
    chain_qa_compatibility: null,
    segments: [
      segment(0, 'row-long-0', 'Earlier discovery', [
        textMessage('long-older-target', 0, 'user', 'Older deep-link target from the real cursor page.'),
        textMessage('long-older-response', 0, 'agent', 'The older segment is merged through the page cursor path.'),
      ], 'A historical handoff boundary survives pagination.'),
    ],
    has_older: false,
    before: null,
  });
}

export const productConversationScenarios = [
  {
    ...productConversationScenarioDefinitions[0],
    snapshot: makeSnapshot(),
    chain: makeChain(),
  },
  {
    ...productConversationScenarioDefinitions[1],
    snapshot: makeSnapshot({
      presentation: { kind: 'state', display_name: 'Product Alpha mobile', presentation_mode: 'idle' },
      latest_transcript_row_id: 'row-mobile-1',
      writable_transcript_row_id: 'row-mobile-1',
      source: null,
      work_identity: null,
      chain_qa_compatibility: { root_transcript_row_id: 'chain-root-product-alpha', url: '/chains/chain-root-product-alpha' },
      segments: [
        segment(1, 'row-mobile-1', 'Mobile root', [
          textMessage('mobile-1', 1, 'user', 'Show how this page stacks on a phone.'),
          textMessage('mobile-2', 2, 'agent', 'The fixture uses the real ProductConversationPage and a read-only history shell.', state('idle')),
        ], 'The desktop investigation narrowed the mobile fixture to a compact transcript shell.'),
      ],
    }),
    chain: makeChain(),
  },
  {
    ...productConversationScenarioDefinitions[2],
    snapshot: makeSnapshot({
      presentation: { kind: 'state', display_name: 'Exhausted aggregate', presentation_mode: 'context_exhausted' },
      latest_transcript_row_id: 'row-exhausted',
      writable_transcript_row_id: 'row-exhausted',
      source: null,
      work_identity: null,
      chain_qa_compatibility: null,
      segments: [
        segment(1, 'row-exhausted', 'Exhausted latest segment', [
          textMessage('exhausted-1', 1, 'user', 'Finish the implementation before context fills.'),
          textMessage('exhausted-2', 2, 'agent', 'The implementation is ready for a fresh context.'),
        ], null),
      ],
    }),
    latestConversationState: {
      type: 'context_exhausted',
      summary: 'Preserve ProductConversation continuation controls and verify the mobile journey.',
    },
    chain: makeChain(),
  },
  {
    ...productConversationScenarioDefinitions[3],
    snapshot: makeSnapshot({
      presentation: { kind: 'state', display_name: 'Compacting aggregate', presentation_mode: 'working' },
      latest_transcript_row_id: 'row-compacting',
      writable_transcript_row_id: 'row-compacting',
      source: null,
      work_identity: null,
      chain_qa_compatibility: null,
      segments: [
        segment(1, 'row-compacting', 'Compacting latest segment', [
          textMessage('compacting-1', 1, 'agent', 'Preparing a fresh-context handoff.'),
        ], null),
      ],
    }),
    latestConversationState: { type: 'awaiting_continuation', attempt: 1 },
    chain: makeChain(),
  },
  {
    ...productConversationScenarioDefinitions[4],
    snapshot: makeSnapshot({
      ordinary_lifecycle: 'history',
      writable_transcript_row_id: null,
      presentation: { kind: 'state', display_name: 'Archived product history', presentation_mode: 'done' },
      work_identity: null,
      source: {
        status: 'deleted',
        source_product_conversation_id: 'pc-deleted',
        source_conversation_id: 'conv-deleted',
        relation: 'approved_task',
        relation_key: 'task-39751',
      },
      chain_qa_compatibility: { root_transcript_row_id: 'chain-root-product-alpha', url: '/chains/chain-root-product-alpha' },
      segments: [
        segment(1, 'row-history-1', 'Historical root', [
          textMessage('history-1', 1, 'user', 'What happened before the handoff?'),
          textMessage('history-2', 2, 'agent', 'A prior worktree produced the approved task and was archived after handoff.'),
        ], 'Historical summary from the predecessor transcript.'),
        segment(2, 'row-history-2', 'Historical continuation', [
          textMessage('history-3', 3, 'user', 'Why is the composer missing?'),
          textMessage('history-4', 4, 'agent', 'History snapshots remain read-only even when Q&A history is visible.', state('terminal')),
        ], null),
      ],
    }),
    chain: makeChain(),
  },
  {
    ...productConversationScenarioDefinitions[5],
  },
  {
    ...productConversationScenarioDefinitions[6],
    initialSnapshotFailures: 1,
    snapshotError: 'Fixture failed to fetch product conversation snapshot',
    snapshot: makeSnapshot({
      presentation: { kind: 'state', display_name: 'Recovered fixture conversation', presentation_mode: 'idle' },
      writable_transcript_row_id: 'row-work',
    }),
  },
  {
    ...productConversationScenarioDefinitions[7],
    snapshot: alignedPrefixScenario.snapshot,
    olderSnapshot: alignedPrefixScenario.olderSnapshot,
    alignedLatestMessages: alignedPrefixScenario.alignedLatestMessages,
    chain: makeChain(),
  },
  {
    ...productConversationScenarioDefinitions[8],
    snapshot: makeLongSnapshot(),
    olderSnapshot: makeOlderPage(),
  },
] as const satisfies readonly ProductConversationScenario[];

export function getProductConversationScenario(id: ProductConversationScenarioId): ProductConversationScenario {
  const scenario = productConversationScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown ProductConversation scenario: ${id}`);
  return scenario;
}
