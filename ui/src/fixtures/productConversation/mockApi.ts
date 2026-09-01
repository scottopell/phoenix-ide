import { api, streamApi, type ChainSseEventData, type Conversation, type Message } from '../../api';
import type { ProductConversationScenario } from './types';

function latestSegmentConversation(scenario: ProductConversationScenario): { conversation: Conversation; messages: Message[]; agent_working: boolean; presentation_mode: string; context_window_size: number } {
  const snapshot = scenario.snapshot;
  if (!snapshot) throw new Error(`Scenario ${scenario.id} is missing snapshot data`);
  const latestSegment = snapshot.segments.find((segment) => segment.transcript_row_id === snapshot.latest_transcript_row_id)
    ?? snapshot.segments[snapshot.segments.length - 1];
  if (!latestSegment) throw new Error(`Scenario ${scenario.id} has no latest segment`);
  return {
    conversation: {
      id: latestSegment.transcript_row_id,
      slug: latestSegment.slug ?? latestSegment.transcript_row_id,
      model: 'claude-sonnet-4-6',
      cwd: snapshot.work_identity?.worktree_path ?? '/fixture/product-conversation',
      created_at: latestSegment.messages[0]?.created_at ?? snapshot.updated_at,
      updated_at: snapshot.updated_at,
      message_count: latestSegment.messages.length,
      state: { type: 'idle' },
      branch_name: snapshot.work_identity?.branch_name ?? null,
      base_branch: snapshot.work_identity?.base_branch ?? null,
      worktree_path: snapshot.work_identity?.worktree_path ?? null,
      task_title: snapshot.work_identity?.task_title ?? null,
      conv_mode_label: 'Work',
      browser_session_active: false,
      terminal_uses_tmux: false,
      work_scope_key: `conversation:${latestSegment.transcript_row_id}`,
    },
    messages: latestSegment.messages.map((message): Message => ({
      message_id: message.message_id,
      conversation_id: latestSegment.transcript_row_id,
      sequence_id: message.sequence_id,
      message_type: message.message_type,
      content: message.content as Message['content'],
      display_data: (message.display_data ?? null) as NonNullable<Message['display_data']> | null,
      usage_data: message.usage_data ?? null,
      created_at: message.created_at,
    })),
    agent_working: false,
    presentation_mode: snapshot.presentation.kind === 'state'
      ? snapshot.presentation.presentation_mode
      : snapshot.presentation.kind,
    context_window_size: 200_000,
  };
}

export function installProductConversationFixtureApi(scenario: ProductConversationScenario): () => void {
  const OriginalEventSource = globalThis.EventSource;
  const originalGetProductConversationSnapshot = api.getProductConversationSnapshot;
  const originalGetChain = api.getChain;
  const originalSubmitChainQuestion = api.submitChainQuestion;
  const originalGetConversationBySlug = api.getConversationBySlug;
  const originalGetConversation = api.getConversation;
  const originalGetConversationRoute = api.getConversationRoute;
  const originalGetConversationRouteBySlug = api.getConversationRouteBySlug;
  const originalGetConversationMessagesLatest = api.getConversationMessagesLatest;
  const originalGetConversationMessagesBefore = api.getConversationMessagesBefore;
  const originalDismissError = api.dismissError;
  const originalReconcileAcceptedMessages = api.reconcileAcceptedMessages;
  const originalListModels = api.listModels;
  const originalGetPrStatus = api.getPrStatus;
  const originalGetConversationGitStatus = api.getConversationGitStatus;
  const originalListForkProposals = api.listForkProposals;
  const originalResolveCoordinatorRoute = api.resolveCoordinatorRoute;
  const originalSubscribeToChainStream = streamApi.subscribeToChainStream;

  class FixtureEventSource extends EventTarget {
    static readonly CONNECTING = 0;
    static readonly OPEN = 1;
    static readonly CLOSED = 2;
    readonly CONNECTING = 0;
    readonly OPEN = 1;
    readonly CLOSED = 2;
    readonly url: string;
    readonly withCredentials = false;
    readyState = FixtureEventSource.OPEN;
    onopen: ((this: EventSource, ev: Event) => unknown) | null = null;
    onmessage: ((this: EventSource, ev: MessageEvent) => unknown) | null = null;
    onerror: ((this: EventSource, ev: Event) => unknown) | null = null;
    constructor(url: string | URL) {
      super();
      this.url = String(url);
      const transcriptId = scenario.snapshot?.latest_transcript_row_id;
      const fixtureConversation = latestSegmentConversation(scenario);
      if (transcriptId) {
        queueMicrotask(() => {
          this.dispatchEvent(new Event('open'));
          this.dispatchEvent(new MessageEvent('init', { data: JSON.stringify({
            sequence_id: 0,
            conversation: fixtureConversation.conversation,
            transcript_generation: 1,
            transcript_coverage: 'complete',
            messages: fixtureConversation.messages,
            steering_messages: [],
            agent_working: false,
            last_sequence_id: fixtureConversation.messages.at(-1)?.sequence_id ?? 0,
            stream_incarnation: `fixture-${transcriptId}`,
            presentation_mode: fixtureConversation.presentation_mode,
            context_window_size: fixtureConversation.context_window_size,
            project_name: null,
            pending_anchor_sequence_id: 0,
            pending_events: [],
            pending_truncated: false,
          }) }));
        });
      }
    }
    close() { this.readyState = FixtureEventSource.CLOSED; }
  }
  globalThis.EventSource = FixtureEventSource as unknown as typeof EventSource;

  api.getProductConversationSnapshot = async () => {
    if (scenario.state === 'loading') {
      return new Promise(() => {}) as ReturnType<typeof api.getProductConversationSnapshot>;
    }
    if (scenario.state === 'error') {
      throw new Error(scenario.snapshotError ?? 'Fixture failed to fetch product conversation snapshot');
    }
    if (!scenario.snapshot) {
      throw new Error(`Scenario ${scenario.id} is missing snapshot data`);
    }
    return scenario.snapshot;
  };

  api.getChain = async () => {
    if (!scenario.chain) {
      throw new Error(`Scenario ${scenario.id} does not provide chain data`);
    }
    return scenario.chain;
  };

  api.submitChainQuestion = async () => ({ chain_qa_id: 'fixture-chain-qa' });
  api.getConversation = async () => latestSegmentConversation(scenario);
  api.getConversationBySlug = async () => latestSegmentConversation(scenario);
  api.getConversationRoute = async (id: string) => ({ id, slug: id, canonical_route: `/c/${encodeURIComponent(id)}` });
  api.getConversationRouteBySlug = async (slug: string) => {
    const id = scenario.snapshot?.latest_transcript_row_id ?? slug;
    return { id, slug, canonical_route: `/c/${encodeURIComponent(slug)}` };
  };
  api.getConversationMessagesLatest = async () => ({
    messages: latestSegmentConversation(scenario).messages,
    tombstones: [],
    transcript_generation: 1,
    server_message_tail: null,
    has_older_messages: false,
  });
  api.getConversationMessagesBefore = async () => ({
    messages: [],
    tombstones: [],
    transcript_generation: 1,
    server_message_tail: null,
    has_older_messages: false,
  });
  api.dismissError = async () => ({ success: true });
  api.reconcileAcceptedMessages = async () => ({ conversation_idle: true, entries: [] });
  api.listModels = async () => ({ models: [], default: 'mock', llm_configured: true, credential_status: 'valid' });
  api.getPrStatus = async () => ({
    found: false,
    refresh: { state: 'fresh', last_attempted_at: '2026-07-01T12:00:00Z', stale: false },
    work_change: { kind: 'clean' },
  });
  api.getConversationGitStatus = async () => ({ kind: 'non_git' });
  api.listForkProposals = async () => [];
  api.resolveCoordinatorRoute = async () => ({ coordinator_id: null });

  streamApi.subscribeToChainStream = (
    rootId: string,
    onEvent: (event: ChainSseEventData) => void,
    onError?: (err: unknown) => void,
  ) => {
    void rootId;
    void onEvent;
    void onError;
    return { close() {} } as EventSource;
  };

  return () => {
    globalThis.EventSource = OriginalEventSource;
    api.getProductConversationSnapshot = originalGetProductConversationSnapshot;
    api.getChain = originalGetChain;
    api.submitChainQuestion = originalSubmitChainQuestion;
    api.getConversation = originalGetConversation;
    api.getConversationBySlug = originalGetConversationBySlug;
    api.getConversationRoute = originalGetConversationRoute;
    api.getConversationRouteBySlug = originalGetConversationRouteBySlug;
    api.getConversationMessagesLatest = originalGetConversationMessagesLatest;
    api.getConversationMessagesBefore = originalGetConversationMessagesBefore;
    api.dismissError = originalDismissError;
    api.reconcileAcceptedMessages = originalReconcileAcceptedMessages;
    api.listModels = originalListModels;
    api.getPrStatus = originalGetPrStatus;
    api.getConversationGitStatus = originalGetConversationGitStatus;
    api.listForkProposals = originalListForkProposals;
    api.resolveCoordinatorRoute = originalResolveCoordinatorRoute;
    streamApi.subscribeToChainStream = originalSubscribeToChainStream;
  };
}
