import { api, type Conversation, type Message } from '../../api';
import type { GoalControl, GoalEntry } from '../../goalApi';
import { coordinatorSnapshot } from './scenarios';
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
    archived: scenario.kind !== 'coordinator' && scenario.snapshot?.ordinary_lifecycle === 'history',
    browser_session_active: false,
    terminal_uses_tmux: false,
    work_scope_key: scenario.kind === 'coordinator' ? 'global:' : 'fixture:latest-member',
  };
}

export function installProductConversationFixtureApi(scenario: ProductConversationScenario): () => void {
  const originalGetProductConversationSnapshot = api.getProductConversationHistorySnapshot;
  const originalDefaultSnapshot = api.getProductConversationSnapshot;
  const originalGetCoordinatorHistory = api.getCoordinatorHistory;
  const originalWatches = api.listActiveCoordinatorWatches;
  const originalBash = api.listLiveCoordinatorBashHandles;
  const originalGetChain = api.getChain;
  const originalSubmitChainQuestion = api.submitChainQuestion;
  const originalGetPrStatus = api.getPrStatus;
  const originalGetConversationRoute = api.getConversationRoute;
  const originalGetConversationRouteBySlug = api.getConversationRouteBySlug;
  const originalGetConversation = api.getConversation;
  const originalResolveCoordinatorRoute = api.resolveCoordinatorRoute;
  const originalEnsureGlobalCoordinator = api.ensureGlobalCoordinator;
  const originalListProductConversations = api.listProductConversations;
  const originalGetProductAutomaticContinuation = api.getProductConversationAutomaticContinuation;
  const originalGetCoordinatorAutomaticContinuation = api.getCoordinatorAutomaticContinuation;
  const originalListConversations = api.listConversations;
  const originalListArchivedConversations = api.listArchivedConversations;
  const originalListModels = api.listModels;
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
  let releaseOlderSnapshot: (() => void) | null = null;

  const record = (name: string, value: number | string) => {
    document.documentElement.dataset[`productConversationFixture${name}`] = String(value);
  };

  let heldOlderPage: Promise<void> | null = null;
  let releaseHeldOlderPage: (() => void) | null = null;
  const holdOlderPage = () => {
    if (!heldOlderPage) heldOlderPage = new Promise<void>(resolve => { releaseHeldOlderPage = resolve; });
  };
  const releaseOlderPage = () => {
    releaseHeldOlderPage?.();
    heldOlderPage = null;
    releaseHeldOlderPage = null;
  };
  window.addEventListener('product-conversation-fixture-hold-older', holdOlderPage);
  window.addEventListener('product-conversation-fixture-release-older', releaseOlderPage);

  api.getProductConversationHistorySnapshot = async (_productConversationId, options = {}) => {
    if (scenario.kind === 'coordinator') throw new Error('Global must not request ordinary history');
    if (scenario.sourceSnapshot && _productConversationId !== 'fixture-product-conversation') {
      if (scenario.sourceIsGlobal) throw new Error("Global must not request ordinary snapshot");
      return scenario.sourceSnapshot;
    }
    snapshotRequestCount += 1;
    record('SnapshotRequests', snapshotRequestCount);
    if (scenario.state === 'loading') {
      return new Promise(() => {}) as ReturnType<typeof api.getProductConversationHistorySnapshot>;
    }
    if (initialSnapshotFailuresRemaining > 0) {
      initialSnapshotFailuresRemaining -= 1;
      throw new Error(scenario.snapshotError ?? 'Fixture failed to fetch product conversation snapshot');
    }
    if (!scenario.snapshot) {
      throw new Error(`Scenario ${scenario.id} is missing snapshot data`);
    }
    if (options.before) {
      const older = scenario.olderPages?.[options.before] ?? (options.before === scenario.snapshot.before ? scenario.olderSnapshot : undefined);
      if (!older) {
        throw new Error(`Scenario ${scenario.id} received an unexpected older-history cursor`);
      }
      record('OlderSnapshotRequests', Number(document.documentElement.dataset['productConversationFixtureOlderSnapshotRequests'] ?? '0') + 1);
      if (scenario.id === 'latest-row-aligned-prefix-tail') {
        record('OlderSnapshotPending', 'true');
        await new Promise<void>((resolve) => {
          releaseOlderSnapshot = resolve;
        });
        record('OlderSnapshotPending', 'false');
      }
      await heldOlderPage;
      return older;
    }
    return scenario.snapshot;
  };

  api.getProductConversationSnapshot = async (reference, options = {}) => {
    const history = await api.getProductConversationHistorySnapshot(reference, options);
    const { boundaries, segments, ...metadata } = history;
    const pages = scenario.kind === 'coordinator' ? [] : [scenario.snapshot, scenario.olderSnapshot, ...Object.values(scenario.olderPages ?? {})];
    return { ...metadata, segments: segments.map(segment => {
      const boundary = boundaries.find(candidate => candidate.predecessor_transcript_row_id === segment.transcript_row_id);
      const message = boundary && [history, scenario.sourceSnapshot, ...pages].flatMap(page => page?.segments.flatMap(segment => segment.messages) ?? []).find(message => message.message_id === boundary.message_id);
      const summary = message && typeof message.content === 'object' && message.content !== null && 'summary' in message.content ? message.content.summary : undefined;
      let handoff = null;
      if (boundary) {
        if (typeof summary !== 'string') throw new Error(`Fixture handoff summary unavailable: ${boundary.message_id}`);
        handoff = { kind: 'historical' as const, predecessor_transcript_row_id: boundary.predecessor_transcript_row_id, successor_transcript_row_id: boundary.successor_transcript_row_id, continuation_message_id: boundary.message_id, summary };
      }
      return { ...segment,
        messages: segment.messages.filter(message => message.message_id !== boundary?.message_id),
        handoff,
      };
    }) };
  };

  api.getCoordinatorHistory = async (reference, options = {}) => {
    if (scenario.kind !== 'coordinator') {
      if (scenario.sourceIsGlobal && scenario.sourceSnapshot) return coordinatorSnapshot(scenario.sourceSnapshot);
      throw new Error(`Unexpected Global history reference: ${reference}`);
    }
    if (![scenario.snapshot.product_conversation_id, ...scenario.snapshot.segments.map(segment => segment.transcript_row_id)].includes(reference)) throw new Error(`Unknown Global member: ${reference}`);
    snapshotRequestCount += 1;
    record('SnapshotRequests', snapshotRequestCount);
    if (!options.before) return scenario.snapshot;
    const older = scenario.olderPages?.[options.before];
    if (!older) throw new Error(`Unknown Global cursor: ${options.before}`);
    record('OlderSnapshotRequests', Number(document.documentElement.dataset['productConversationFixtureOlderSnapshotRequests'] ?? '0') + 1);
    await heldOlderPage;
    return older;
  };
  if (!scenario.initialEntry?.startsWith('/global/')) {
    api.listActiveCoordinatorWatches = async () => [];
    api.listLiveCoordinatorBashHandles = async () => [];
  }

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
  const messages = scenario.alignedLatestMessages
    ?? (scenario.sourceSnapshot ? [] : scenario.snapshot?.segments.at(-1)?.messages ?? []) as Message[];

  const sourceId = scenario.sourceSnapshot?.segments[0]?.transcript_row_id;
  const sourceConversation = { ...conversation, id: sourceId ?? conversation.id, slug: sourceId ?? conversation.slug };
  const sourceMessages = (scenario.sourceSnapshot?.segments[0]?.messages ?? []) as Message[];
  api.getConversationRoute = async (id) => id === sourceId ? { id, slug: id } : route;
  api.getConversationRouteBySlug = async (id) => id === sourceId ? { id, slug: id } : route;
  api.getConversation = async (id) => ({
    conversation: id === sourceId ? sourceConversation : scenario.snapshot?.segments.some(segment => segment.transcript_row_id === id)
      ? { ...conversation, id, slug: id } : conversation,
    messages: id === sourceId ? sourceMessages : (scenario.snapshot?.segments.find(segment => segment.transcript_row_id === id)?.messages as Message[] | undefined) ?? messages,
    agent_working: false,
    presentation_mode: 'idle',
    context_window_size: 128_000,
  });
  api.listProductConversations = async () => ({ product_conversations: [] });
  api.getProductConversationAutomaticContinuation = async (id) => ({ aggregate: { kind: 'ordinary', product_conversation_id: id }, auto_continue_on_context_exhaustion: false, admission: null });
  api.getCoordinatorAutomaticContinuation = async () => ({ aggregate: { kind: 'coordinator', product_conversation_id: 'source-product' }, auto_continue_on_context_exhaustion: false, admission: null });
  api.ensureGlobalCoordinator = async () => ({ conversation: scenario.kind === 'coordinator' ? conversation : { ...sourceConversation, id: 'current-global', slug: 'current-global' }, created: false });
  api.resolveCoordinatorRoute = async (id) => ({ coordinator_id: scenario.kind === 'coordinator' && scenario.snapshot.segments.some(segment => segment.transcript_row_id === id) ? scenario.snapshot.product_conversation_id : scenario.sourceIsGlobal && (id === sourceId || id === 'current-global') ? scenario.sourceSnapshot?.product_conversation_id ?? null : scenario.initialEntry === `/global/${id}` ? id : null });
  api.listConversations = async () => [];
  api.listArchivedConversations = async () => [];
  api.listModels = async () => ({
    models: [],
    default: '',
    llm_configured: false,
    credential_status: 'not_configured',
  });
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
  const releaseOlderSnapshotResponse = () => {
    releaseOlderSnapshot?.();
    releaseOlderSnapshot = null;
  };
  window.addEventListener('product-conversation-fixture-release-older', releaseOlderSnapshotResponse);

  globalThis.fetch = async (input, init) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    if (url.endsWith('/api/telemetry/conversation-open') || url.endsWith('/api/telemetry/product-conversation-open')) {
      return new Response(null, { status: 204 });
    }
    if (url.endsWith(`/api/conversations/${conversation.id}/chat`) && init?.method === 'POST') {
      const body = JSON.parse(String(init.body)) as { text?: string };
      sendCount += 1;
      record('SendCount', sendCount);
      record('LastSentText', body.text ?? '');
      return Response.json({ queued: false, already_persisted: true });
    }
    if ((init?.method ?? 'GET') === 'GET' && scenario.initialEntry?.startsWith('/global/')) {
      if (url === '/api/coordinator/watches') {
        record('WatchRequests', Number(document.documentElement.dataset['productConversationFixtureWatchRequests'] ?? '0') + 1);
        return Response.json([] satisfies Awaited<ReturnType<typeof api.listActiveCoordinatorWatches>>);
      }
      if (url === '/api/coordinator/bash/live') {
        record('LiveBashRequests', Number(document.documentElement.dataset['productConversationFixtureLiveBashRequests'] ?? '0') + 1);
        return Response.json([] satisfies Awaited<ReturnType<typeof api.listLiveCoordinatorBashHandles>>);
      }
    }
    const method = (init?.method ?? (input instanceof Request ? input.method : 'GET')).toUpperCase();
    const goalRoot = scenario.kind !== 'coordinator' && scenario.snapshot
      ? `/api/product-conversations/${encodeURIComponent(scenario.snapshot.product_conversation_id)}`
      : null;
    const requestUrl = new URL(url, location.href);
    if (method === 'GET' && goalRoot) {
      if (requestUrl.pathname === `${goalRoot}/goal` && !requestUrl.search) {
        return Response.json(null satisfies GoalEntry | null);
      }
      if (requestUrl.pathname === `${goalRoot}/goal-control` && !requestUrl.search) {
        return Response.json(null satisfies GoalControl | null);
      }
      if (requestUrl.pathname === `${goalRoot}/goal/history`
        && /^\?after=(0|[1-9]\d*)$/.test(requestUrl.search)
        && Number.isSafeInteger(Number(requestUrl.searchParams.get('after')))) {
        return Response.json([] satisfies GoalEntry[]);
      }
    }
    if (new URL(url, location.href).pathname.startsWith('/api/')) throw new Error(`Unexpected fixture API request: ${url}`);
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
    private readonly isSource: boolean;

    constructor(url: string) {
      this.isSource = Boolean(sourceId && url.includes(sourceId));
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
          const init = { opening: null,
            sequence_id: 1,
            conversation: this.isSource ? sourceConversation : conversation,
            transcript_generation: 1,
            transcript_coverage: 'complete',
            messages: this.isSource ? sourceMessages : messages,
            steering_messages: scenario.steeringMessages ?? [],
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
    window.removeEventListener('product-conversation-fixture-hold-older', holdOlderPage);
    window.removeEventListener('product-conversation-fixture-release-older', releaseOlderPage);
    releaseOlderPage();
    window.removeEventListener('product-conversation-fixture-stream-failure', failLatestEventSource);
    api.getPrStatus = originalGetPrStatus;
    api.getProductConversationHistorySnapshot = originalGetProductConversationSnapshot;
    api.getProductConversationSnapshot = originalDefaultSnapshot;
    api.getCoordinatorHistory = originalGetCoordinatorHistory;
    api.listActiveCoordinatorWatches = originalWatches;
    api.listLiveCoordinatorBashHandles = originalBash;
    api.getChain = originalGetChain;
    api.submitChainQuestion = originalSubmitChainQuestion;
    api.getConversationRoute = originalGetConversationRoute;
    api.getConversationRouteBySlug = originalGetConversationRouteBySlug;
    api.getConversation = originalGetConversation;
    api.resolveCoordinatorRoute = originalResolveCoordinatorRoute;
    api.getCoordinatorAutomaticContinuation = originalGetCoordinatorAutomaticContinuation;
    api.listProductConversations = originalListProductConversations;
    api.getProductConversationAutomaticContinuation = originalGetProductAutomaticContinuation;
  api.ensureGlobalCoordinator = originalEnsureGlobalCoordinator;
    api.listConversations = originalListConversations;
    api.listArchivedConversations = originalListArchivedConversations;
    api.listModels = originalListModels;
    api.getSystemPrompt = originalGetSystemPrompt;
    api.getWorkScopeInventory = originalGetWorkScopeInventory;
    api.listForkProposals = originalListForkProposals;
    api.reconcileAcceptedMessages = originalReconcileAcceptedMessages;
    api.continueConversation = originalContinueConversation;
    window.removeEventListener('product-conversation-fixture-release-older', releaseOlderSnapshotResponse);
    globalThis.fetch = originalFetch;
    globalThis.WebSocket = OriginalWebSocket;
    for (const key of Object.keys(document.documentElement.dataset)) {
      if (key.startsWith('productConversationFixture')) delete document.documentElement.dataset[key];
    }
    globalThis.EventSource = OriginalEventSource;
  };
}
