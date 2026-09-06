import { api, type Conversation, type Message } from '../../api';
import type { ProductConversationScenario } from './types';

function latestConversation(scenario: ProductConversationScenario): Conversation {
  const snapshot = scenario.snapshot;
  const id = snapshot?.latest_transcript_row_id ?? 'fixture-latest-member';
  return {
    id,
    slug: id,
    title: 'Fixture latest member',
    model: 'fixture-model',
    cwd: '/fixture',
    created_at: '2026-07-01T12:00:00Z',
    updated_at: '2026-07-01T12:00:00Z',
    message_count: snapshot?.segments.at(-1)?.messages.length ?? 0,
    transcript_generation: 1,
    state: scenario.latestConversationState ?? { type: 'idle' },
    archived: snapshot?.ordinary_lifecycle === 'history',
    browser_session_active: false,
    terminal_uses_tmux: false,
    work_scope_key: 'fixture:latest-member',
  };
}

export function installProductConversationFixtureApi(scenario: ProductConversationScenario): () => void {
  const originalGetProductConversationSnapshot = api.getProductConversationSnapshot;
  const originalGetChain = api.getChain;
  const originalSubmitChainQuestion = api.submitChainQuestion;
  const originalGetPrStatus = api.getPrStatus;
  const originalGetConversationRoute = api.getConversationRoute;
  const originalGetConversationRouteBySlug = api.getConversationRouteBySlug;
  const originalGetConversation = api.getConversation;
  const originalResolveCoordinatorRoute = api.resolveCoordinatorRoute;
  const originalListConversations = api.listConversations;
  const originalListArchivedConversations = api.listArchivedConversations;
  const originalListModels = api.listModels;
  const originalGetWakeStatus = api.getWakeStatus;
  const originalGetSystemPrompt = api.getSystemPrompt;
  const originalGetWorkScopeInventory = api.getWorkScopeInventory;
  const originalListForkProposals = api.listForkProposals;
  const originalReconcileAcceptedMessages = api.reconcileAcceptedMessages;
  const originalContinueConversation = api.continueConversation;
  const originalFetch = globalThis.fetch;
  const OriginalWebSocket = globalThis.WebSocket;
  let snapshotRequestCount = 0;
  let initialSnapshotFailuresRemaining = scenario.initialSnapshotFailures ?? 0;
  let sendCount = 0;
  let eventSourceOpenCount = 0;
  let eventSourceInitCount = 0;
  let latestEventSource: FixtureEventSource | null = null;

  const record = (name: string, value: number | string) => {
    document.documentElement.dataset[`productConversationFixture${name}`] = String(value);
  };

  api.getProductConversationSnapshot = async (_productConversationId, options = {}) => {
    snapshotRequestCount += 1;
    record('SnapshotRequests', snapshotRequestCount);
    if (scenario.state === 'loading') {
      return new Promise(() => {}) as ReturnType<typeof api.getProductConversationSnapshot>;
    }
    if (initialSnapshotFailuresRemaining > 0) {
      initialSnapshotFailuresRemaining -= 1;
      throw new Error(scenario.snapshotError ?? 'Fixture failed to fetch product conversation snapshot');
    }
    if (!scenario.snapshot) {
      throw new Error(`Scenario ${scenario.id} is missing snapshot data`);
    }
    if (options.before) {
      if (!scenario.olderSnapshot || options.before !== scenario.snapshot.before) {
        throw new Error(`Scenario ${scenario.id} received an unexpected older-history cursor`);
      }
      record('OlderSnapshotRequests', Number(document.documentElement.dataset['productConversationFixtureOlderSnapshotRequests'] ?? '0') + 1);
      return scenario.olderSnapshot;
    }
    return scenario.snapshot;
  };

  api.getChain = async (rootConvId) => {
    if (!scenario.chain) throw new Error(`Scenario ${scenario.id} is missing Recall data`);
    return { ...scenario.chain, root_conv_id: rootConvId };
  };
  api.submitChainQuestion = async () => ({ chain_qa_id: 'fixture-recall-new' });

  api.getPrStatus = async () => ({
    found: true as const,
    number: 750,
    display_state: 'open' as const,
    check_state: 'passing' as const,
    feedback_freshness: { state: 'new' as const, count: 2 },
    refresh: { state: 'fresh' as const, stale: false, last_attempted_at: '2026-07-01T12:00:00Z' },
    work_change: { kind: 'clean' as const },
  });

  const conversation = latestConversation(scenario);
  const route = { id: conversation.id, slug: conversation.slug };
  const messages = (scenario.snapshot?.segments.at(-1)?.messages ?? []) as Message[];

  api.getConversationRoute = async () => route;
  api.getConversationRouteBySlug = async () => route;
  api.getConversation = async () => ({
    conversation,
    messages,
    agent_working: false,
    presentation_mode: 'idle',
    context_window_size: 128_000,
  });
  api.resolveCoordinatorRoute = async () => ({ coordinator_id: null });
  api.listConversations = async () => [];
  api.listArchivedConversations = async () => [];
  api.listModels = async () => ({
    models: [],
    default: '',
    llm_configured: false,
    credential_status: 'not_configured',
  });
  api.getWakeStatus = async () => ({ pending_count: 0, soonest_expires_at: null, contracts: [] });
  api.getSystemPrompt = async () => '';
  api.getWorkScopeInventory = async () => ({
    scope_key: conversation.work_scope_key ?? 'fixture:latest-member',
    bash: [],
    tmux: null,
    browser: null,
  });
  api.listForkProposals = async () => [];
  api.reconcileAcceptedMessages = async (_conversationId, messageIds) => ({
    conversation_idle: false,
    entries: messageIds.map((messageId) => ({
      message_id: messageId,
      status: 'steering_queued' as const,
    })),
  });
  api.continueConversation = async () => ({
    conversation_id: 'fixture-successor',
    slug: 'fixture-successor',
    status: 'accepted',
  });
  globalThis.fetch = async (input, init) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    if (url.endsWith('/api/telemetry/conversation-open')) {
      return new Response(null, { status: 204 });
    }
    if (url.endsWith(`/api/conversations/${conversation.id}/chat`) && init?.method === 'POST') {
      const body = JSON.parse(String(init.body)) as { text?: string };
      sendCount += 1;
      record('SendCount', sendCount);
      record('LastSentText', body.text ?? '');
      return Response.json({ queued: false, already_persisted: true });
    }
    return originalFetch(input, init);
  };

  class FixtureWebSocket {
    static readonly CONNECTING = 0;
    static readonly OPEN = 1;
    static readonly CLOSING = 2;
    static readonly CLOSED = 3;
    readonly CONNECTING = FixtureWebSocket.CONNECTING;
    readonly OPEN = FixtureWebSocket.OPEN;
    readonly CLOSING = FixtureWebSocket.CLOSING;
    readonly CLOSED = FixtureWebSocket.CLOSED;
    readyState = FixtureWebSocket.CONNECTING;
    binaryType: BinaryType = 'blob';
    onopen: ((event: Event) => void) | null = null;
    onmessage: ((event: MessageEvent) => void) | null = null;
    onerror: ((event: Event) => void) | null = null;
    onclose: ((event: CloseEvent) => void) | null = null;

    constructor(url: string | URL) {
      void url;
      queueMicrotask(() => {
        this.readyState = FixtureWebSocket.OPEN;
        this.onopen?.(new Event('open'));
      });
    }

    send(data: string | ArrayBufferLike | Blob | ArrayBufferView): void { void data; }
    close(): void { this.readyState = FixtureWebSocket.CLOSED; }
    addEventListener(): void {}
    removeEventListener(): void {}
    dispatchEvent(): boolean { return true; }
  }
  globalThis.WebSocket = FixtureWebSocket as unknown as typeof WebSocket;

  const OriginalEventSource = globalThis.EventSource;
  class FixtureEventSource {
    static readonly CONNECTING = 0;
    static readonly OPEN = 1;
    static readonly CLOSED = 2;
    readonly CONNECTING = FixtureEventSource.CONNECTING;
    readonly OPEN = FixtureEventSource.OPEN;
    readonly CLOSED = FixtureEventSource.CLOSED;
    readyState = FixtureEventSource.OPEN;
    onerror: ((event: Event) => void) | null = null;
    onopen: ((event: Event) => void) | null = null;
    private readonly listeners = new Map<string, Set<(event: MessageEvent<string>) => void>>();
    private readonly instanceId: number;

    constructor(url: string) {
      eventSourceOpenCount += 1;
      this.instanceId = eventSourceOpenCount;
      latestEventSource = this; // eslint-disable-line @typescript-eslint/no-this-alias
      record('EventSourceOpens', eventSourceOpenCount);
      record('EventSourceLastInstance', this.instanceId);
      record('EventSourceLastUrl', url);
      queueMicrotask(() => this.onopen?.(new Event('open')));
    }

    triggerNativeError(): void {
      this.readyState = FixtureEventSource.CONNECTING;
      const event = new MessageEvent<string>('error', { data: '' });
      this.onerror?.(event);
      for (const listener of this.listeners.get('error') ?? []) listener(event);
    }

    addEventListener(type: string, listener: EventListenerOrEventListenerObject): void {
      const callback = listener as (event: MessageEvent<string>) => void;
      const listeners = this.listeners.get(type) ?? new Set();
      listeners.add(callback);
      this.listeners.set(type, listeners);
      if (type === 'init') {
        queueMicrotask(() => {
          if (this.readyState === FixtureEventSource.CLOSED) return;
          eventSourceInitCount += 1;
          record('EventSourceInits', eventSourceInitCount);
          record('EventSourceLastInitializedInstance', this.instanceId);
          const init = {
            sequence_id: 1,
            conversation,
            transcript_generation: 1,
            transcript_coverage: 'complete',
            messages,
            steering_messages: [],
            agent_working: false,
            last_sequence_id: 1,
            stream_incarnation: `fixture-stream-${this.instanceId}`,
            presentation_mode: 'idle',
            context_window_size: 128_000,
            project_name: null,
            pending_anchor_sequence_id: 1,
            pending_events: [],
            pending_truncated: false,
          };
          callback(new MessageEvent('init', { data: JSON.stringify(init) }));
        });
      }
    }

    removeEventListener(type: string, listener: EventListenerOrEventListenerObject): void {
      this.listeners.get(type)?.delete(listener as (event: MessageEvent<string>) => void);
    }

    close(): void { this.readyState = FixtureEventSource.CLOSED; }
  }
  globalThis.EventSource = FixtureEventSource as unknown as typeof EventSource;
  const failLatestEventSource = () => latestEventSource?.triggerNativeError();
  window.addEventListener('product-conversation-fixture-stream-failure', failLatestEventSource);

  return () => {
    window.removeEventListener('product-conversation-fixture-stream-failure', failLatestEventSource);
    api.getPrStatus = originalGetPrStatus;
    api.getProductConversationSnapshot = originalGetProductConversationSnapshot;
    api.getChain = originalGetChain;
    api.submitChainQuestion = originalSubmitChainQuestion;
    api.getConversationRoute = originalGetConversationRoute;
    api.getConversationRouteBySlug = originalGetConversationRouteBySlug;
    api.getConversation = originalGetConversation;
    api.resolveCoordinatorRoute = originalResolveCoordinatorRoute;
    api.listConversations = originalListConversations;
    api.listArchivedConversations = originalListArchivedConversations;
    api.listModels = originalListModels;
    api.getWakeStatus = originalGetWakeStatus;
    api.getSystemPrompt = originalGetSystemPrompt;
    api.getWorkScopeInventory = originalGetWorkScopeInventory;
    api.listForkProposals = originalListForkProposals;
    api.reconcileAcceptedMessages = originalReconcileAcceptedMessages;
    api.continueConversation = originalContinueConversation;
    globalThis.fetch = originalFetch;
    globalThis.WebSocket = OriginalWebSocket;
    for (const key of Object.keys(document.documentElement.dataset)) {
      if (key.startsWith('productConversationFixture')) delete document.documentElement.dataset[key];
    }
    globalThis.EventSource = OriginalEventSource;
  };
}
