import { api, ApiResponseError, type Conversation, type Message, type ProductConversationHistorySnapshotView } from '../../api';
import { coordinatorSnapshot } from './scenarios';
import type { ProductConversationScenario } from './types';

function isOrdinarySnapshot(snapshot: NonNullable<ProductConversationScenario['snapshot']>): snapshot is ProductConversationHistorySnapshotView {
  return 'close' in snapshot;
}

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
  const originalUploadAttachments = api.uploadAttachments;
  const originalContinueConversation = api.continueConversation;
  const originalFetch = globalThis.fetch;
  const OriginalWebSocket = globalThis.WebSocket;
  let snapshotRequestCount = 0;
  let initialSnapshotFailuresRemaining = scenario.initialSnapshotFailures ?? 0;
  let sendCount = 0;
  let eventSourceOpenCount = 0;
  let eventSourceInitCount = 0;
  let latestEventSource: FixtureEventSource | null = null;
  let lastSubmittedMessage: Message | null = null;
  let currentSnapshot = scenario.snapshot;
  let staleSnapshot: typeof currentSnapshot;
  const holdStaleSnapshot = () => { staleSnapshot = currentSnapshot; };
  window.addEventListener('product-conversation-fixture-stale-snapshot', holdStaleSnapshot);
  let currentConversation = latestConversation(scenario);
  let currentMessages = scenario.alignedLatestMessages
    ?? (scenario.snapshot?.segments.at(-1)?.messages ?? []) as Message[];
  let continuationPublished = false;
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
  let holdSuccessorInit = false;
  const heldSuccessorInits: Array<() => void> = [];
  const holdSuccessor = () => { holdSuccessorInit = true; };
  const releaseSuccessor = () => {
    holdSuccessorInit = false;
    for (const release of heldSuccessorInits.splice(0)) release();
  };
  window.addEventListener('product-conversation-fixture-hold-successor', holdSuccessor);
  window.addEventListener('product-conversation-fixture-release-successor', releaseSuccessor);
  let holdSuccessorReadback = false;
  let releaseSuccessorReadback: (() => void) | null = null;
  const holdSuccessorConversationRead = () => { holdSuccessorReadback = true; };
  const releaseSuccessorConversationRead = () => { holdSuccessorReadback = false; releaseSuccessorReadback?.(); releaseSuccessorReadback = null; };
  window.addEventListener('product-conversation-fixture-hold-successor-readback', holdSuccessorConversationRead);
  window.addEventListener('product-conversation-fixture-release-successor-readback', releaseSuccessorConversationRead);
  let resolveUpload: ((files: Awaited<ReturnType<typeof api.uploadAttachments>>) => void) | null = null;
  let holdUpload = false;
  const holdFixtureUpload = () => { holdUpload = true; };
  api.uploadAttachments = async (conversationId, files) => holdUpload
    ? await new Promise(resolve => { resolveUpload = resolve; })
    : originalUploadAttachments(conversationId, files);
  window.addEventListener('product-conversation-fixture-hold-upload', holdFixtureUpload);
  const completeUpload = () => resolveUpload?.([{ original_name: 'notes.txt', media_type: 'text/plain', size_bytes: 4, stored_path: 'notes.txt' }]);
  window.addEventListener('product-conversation-fixture-complete-upload', completeUpload);

  let snapshotUnavailable = false;
  const failSnapshot = () => { snapshotUnavailable = true; };
  const restoreSnapshot = () => { snapshotUnavailable = false; staleSnapshot = undefined; };
  const checkSnapshotFailure = () => {
    if (!snapshotUnavailable) return;
    throw new Error('Fixture continuation snapshot unavailable');
  };
  window.addEventListener('product-conversation-fixture-fail-snapshot', failSnapshot);
  window.addEventListener('product-conversation-fixture-restore-snapshot', restoreSnapshot);
  api.getProductConversationHistorySnapshot = async (_productConversationId, options = {}) => {
    const requestOptions = options ?? {};
    if (!requestOptions.before) checkSnapshotFailure();
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
    if (requestOptions.before) {
      const older = scenario.olderPages?.[requestOptions.before] ?? (requestOptions.before === scenario.snapshot.before ? scenario.olderSnapshot : undefined);
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
    const snapshot = staleSnapshot ?? currentSnapshot;
    if (!snapshot || !isOrdinarySnapshot(snapshot)) throw new Error(`Scenario ${scenario.id} has no ordinary snapshot`);
    return snapshot;
  };

  api.getProductConversationSnapshot = async (reference, options = {}) => {
    if (scenario.aliasFallbackNotFound && reference === scenario.snapshot?.latest_transcript_row_id) {
      throw new ApiResponseError('Conversation aggregate not found', 404);
    }
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
    if (!options.before) checkSnapshotFailure();
    if (scenario.kind !== 'coordinator') {
      if (scenario.sourceIsGlobal && scenario.sourceSnapshot) return coordinatorSnapshot(scenario.sourceSnapshot);
      throw new Error(`Unexpected Global history reference: ${reference}`);
    }
    if (![scenario.snapshot.product_conversation_id, ...scenario.snapshot.segments.map(segment => segment.transcript_row_id)].includes(reference)) throw new Error(`Unknown Global member: ${reference}`);
    snapshotRequestCount += 1;
    record('SnapshotRequests', snapshotRequestCount);
    if (!options.before) return (staleSnapshot ?? currentSnapshot) as typeof scenario.snapshot;
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

  const conversation = currentConversation;
  let predecessorConversation: Conversation | null = null;
  let route = { id: conversation.id, slug: conversation.slug };

  const sourceId = scenario.sourceSnapshot?.segments[0]?.transcript_row_id;
  const sourceConversation = { ...conversation, id: sourceId ?? conversation.id, slug: sourceId ?? conversation.slug };
  const sourceMessages = (scenario.sourceSnapshot?.segments[0]?.messages ?? []) as Message[];
  api.getConversationRoute = async (id) => id === sourceId ? { id, slug: id } : route;
  api.getConversationRouteBySlug = async (id) => id === sourceId ? { id, slug: id } : route;
  api.getConversation = async (id) => {
    if (holdSuccessorReadback && id === currentConversation.id) {
      await new Promise<void>(resolve => { releaseSuccessorReadback = resolve; });
    }
    return {
      conversation: id === sourceId ? sourceConversation : id === predecessorConversation?.id ? predecessorConversation
        : currentSnapshot?.segments.some(segment => segment.transcript_row_id === id)
          ? { ...currentConversation, id, slug: id } : currentConversation,
      messages: id === sourceId ? sourceMessages : (currentSnapshot?.segments.find(segment => segment.transcript_row_id === id)?.messages as Message[] | undefined) ?? currentMessages,
      agent_working: false,
      presentation_mode: 'idle',
      context_window_size: 128_000,
    };
  };
  api.listProductConversations = async () => ({ product_conversations: [], coordinator: null });
  api.getProductConversationAutomaticContinuation = async (id) => ({ aggregate: { kind: 'ordinary', product_conversation_id: id }, auto_continue_on_context_exhaustion: false, admission: null });
  api.getCoordinatorAutomaticContinuation = async () => ({ aggregate: { kind: 'coordinator', product_conversation_id: 'source-product' }, auto_continue_on_context_exhaustion: false, admission: null });
  api.ensureGlobalCoordinator = async () => ({ conversation: scenario.kind === 'coordinator' ? conversation : { ...sourceConversation, id: 'current-global', slug: 'current-global' }, created: false });
  api.resolveCoordinatorRoute = async (id) => ({ coordinator_id: scenario.kind === 'coordinator' && currentSnapshot?.segments.some(segment => segment.transcript_row_id === id) ? scenario.snapshot.product_conversation_id : scenario.sourceIsGlobal && (id === sourceId || id === 'current-global') ? scenario.sourceSnapshot?.product_conversation_id ?? null : scenario.initialEntry === `/global/${id}` ? id : null });
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

  let reminderFailuresRemaining = scenario.reminderFixture?.initialFailures ?? 0;
  let reminderFailuresAfterLoad = scenario.reminderFixture?.failuresAfterFirstLoad ?? 0;
  let hasLoadedReminders = false;
  let reminderRequestCount = 0;
  let reminderMutationCount = 0;
  let conflictRows: import('../../generated/Reminder').Reminder[] | undefined;
  globalThis.fetch = async (input, init) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    const requestUrl = new URL(url, location.href);
    const reminderPath = `/api/conversations/${encodeURIComponent(conversation.id)}/reminders`;
    if (scenario.reminderFixture && requestUrl.pathname === reminderPath && (init?.method ?? 'GET') === 'GET') {
      if (scenario.reminderFixture.reminders.some((row) => row.product_conversation_id !== scenario.snapshot?.product_conversation_id)) {
        throw new Error('Reminders fixture owner differs from product conversation');
      }
      reminderRequestCount += 1;
      record('ReminderRequests', reminderRequestCount);
      if (reminderFailuresRemaining > 0) {
        reminderFailuresRemaining -= 1;
        record('ReminderState', 'failed');
        return Response.json({ error: 'Fixture reminder service unavailable' }, { status: 503 });
      }
      if (hasLoadedReminders && reminderFailuresAfterLoad > 0) {
        reminderFailuresAfterLoad -= 1;
        record('ReminderState', 'failed');
        return Response.json({ error: 'Fixture reminder service unavailable after focus refresh' }, { status: 503 });
      }
      hasLoadedReminders = true;
      record('ReminderState', 'loaded');
      const rows = conflictRows ?? scenario.reminderFixture.reminders;
      record('ReminderServedRevision', rows.find((row) => row.id === 'upcoming-fixture')?.revision ?? 0);
      return Response.json(rows);
    }
    if (scenario.reminderFixture?.rejectMutation && requestUrl.pathname.startsWith(`${reminderPath}/`) && init?.method === 'POST') {
      reminderMutationCount += 1;
      record('ReminderMutations', reminderMutationCount);
      const expectedRevision = (JSON.parse(String(init.body)) as { expected_revision: number }).expected_revision;
      const current = (conflictRows ?? scenario.reminderFixture.reminders).find((row) => row.id === 'upcoming-fixture');
      if (!current || !requestUrl.pathname.endsWith('/upcoming-fixture/cancel')) throw new Error('Unexpected fixture reminder mutation');
      record('LastReminderRevision', expectedRevision);
      if (!conflictRows) {
        conflictRows = scenario.reminderFixture.reminders.map((row) => row.id === current.id
          ? { ...row, revision: row.revision + 1, text: 'Check the release candidate — revised on server' }
          : row);
      }
      const latest = conflictRows.find((row) => row.id === current.id)!;
      if (expectedRevision !== latest.revision) {
        record('ReminderMutation', 'rejected');
        return Response.json({ kind: 'revision_conflict', error: 'The reminder changed in this fixture.', current: latest }, { status: 409 });
      }
      conflictRows = conflictRows.map((row) => row.id === latest.id
        ? { ...row, revision: row.revision + 1, state: { kind: 'cancelled', cancelled_at_us: 1783000000000000 } }
        : row);
      record('ReminderMutation', 'accepted');
      record('ReminderAcceptedRevision', conflictRows.find((row) => row.id === latest.id)?.revision ?? 0);
      return Response.json({ reminder: conflictRows.find((row) => row.id === latest.id) });
    }
    if (url.endsWith('/api/telemetry/conversation-open') || url.endsWith('/api/telemetry/product-conversation-open')) {
      return new Response(null, { status: 204 });
    }
    if (url.endsWith(`/api/conversations/${encodeURIComponent(conversation.id)}/reminders`) && (init?.method ?? 'GET') === 'GET') {
      return Response.json([] satisfies Awaited<ReturnType<typeof api.listReminders>>);
    }
    if (url.endsWith(`/api/conversations/${conversation.id}/chat`) && init?.method === 'POST') {
      const body = JSON.parse(String(init.body)) as { text?: string; message_id: string };
      sendCount += 1;
      record('SendCount', sendCount);
      record('LastSentText', body.text ?? '');
      lastSubmittedMessage = {
        event_presentation: null, message_id: `${conversation.id}:${body.message_id}`, conversation_id: conversation.id,
        sequence_id: Math.max(0, ...currentMessages.map(message => message.sequence_id)) + 1,
        message_type: 'user', content: { text: body.text ?? '' }, created_at: new Date().toISOString(),
      };
      if (scenario.sendFailure === 'before-acceptance') return Response.json({ error: 'Fixture expansion rejected', error_type: 'file_not_found', reference: '@missing.ts' }, { status: 422 });
      if (scenario.sendFailure === 'after-acceptance') latestEventSource?.emitMessage(lastSubmittedMessage);
      if (scenario.sendFailure) return Response.json({ error: 'Fixture send failure' }, { status: 503 });
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
  const eventSources = new Set<FixtureEventSource>();
  const eventSequenceByMember = new Map<string, number>();
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
    private readonly isTranscript: boolean;
    private readonly conversationId: string;
    get id(): number { return this.instanceId; }
    get memberId(): string { return this.conversationId; }
    nextSequenceId(): number {
      const next = (eventSequenceByMember.get(this.conversationId) ?? 1) + 1;
      eventSequenceByMember.set(this.conversationId, next);
      return next;
    }

    constructor(url: string) {
      this.isSource = Boolean(sourceId && url.includes(sourceId));
      const parsed = new URL(url, location.href);
      this.isTranscript = parsed.pathname === '/api/conversation-reads/stream'
        || /^\/api\/conversations\/[^/]+\/stream$/.test(parsed.pathname);
      this.conversationId = parsed.searchParams.get('reference')
        ?? parsed.pathname.match(/^\/api\/conversations\/([^/]+)\/stream$/)?.[1] ?? '';
      eventSourceOpenCount += 1;
      this.instanceId = eventSourceOpenCount;
      eventSources.add(this);
      latestEventSource = this; // eslint-disable-line @typescript-eslint/no-this-alias
      record('EventSourceOpens', eventSourceOpenCount);
      record('EventSourceLastInstance', this.instanceId);
      record('EventSourceLastUrl', url);
      queueMicrotask(() => this.onopen?.(new Event('open')));
    }

    emitMessage(message: Message): void {
      const sequenceId = Math.max(this.nextSequenceId(), message.sequence_id);
      eventSequenceByMember.set(this.conversationId, sequenceId);
      const event = new MessageEvent<string>('message', { data: JSON.stringify({ opening: null, sequence_id: sequenceId, message: { ...message, sequence_id: sequenceId, origin: { kind: 'user_api' } } }) });
      for (const listener of this.listeners.get('message') ?? []) listener(event);
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
      record('ActiveTranscriptSubscriptions', [...eventSources].filter(source => source.isTranscript && source.subscriptionCount > 0).length);
      if (type === 'init') {
        queueMicrotask(() => {
          if (this.readyState === FixtureEventSource.CLOSED) return;
          eventSourceInitCount += 1;
          record('EventSourceInits', eventSourceInitCount);
          record('EventSourceLastInitializedInstance', this.instanceId);
          const messages = this.isSource ? sourceMessages : this.conversationId === currentConversation.id ? currentMessages : (currentSnapshot?.segments.find(segment => segment.transcript_row_id === this.conversationId)?.messages ?? []);
          const anchor = Math.max(1, eventSequenceByMember.get(this.conversationId) ?? 0, ...messages.map(message => message.sequence_id));
          eventSequenceByMember.set(this.conversationId, anchor);
          const init = { opening: null,
            sequence_id: anchor,
            conversation: this.isSource ? sourceConversation : this.conversationId === predecessorConversation?.id ? predecessorConversation : currentConversation,
            transcript_generation: 1,
            transcript_coverage: 'complete',
            messages,
            steering_messages: scenario.steeringMessages ?? [],
            agent_working: false,
            last_sequence_id: anchor,
            stream_incarnation: `fixture-stream-${this.instanceId}`,
            presentation_mode: 'idle',
            context_window_size: 128_000,
            project_name: null,
            pending_anchor_sequence_id: anchor,
            pending_events: [],
            pending_truncated: false,
          };
          const deliver = () => {
            callback(new MessageEvent('init', { data: JSON.stringify(init) }));
            if (this.isTranscript) {
              record('LastDeliveredInitMember', this.conversationId);
              record('LastDeliveredInitInstance', this.instanceId);
            }
          };
          if (holdSuccessorInit && this.conversationId === currentConversation.id && continuationPublished) heldSuccessorInits.push(deliver);
          else deliver();
        });
      }
    }

    get subscriptionCount(): number { return [...this.listeners.values()].reduce((count, listeners) => count + listeners.size, 0); }

    emit(type: string, data: unknown): void {
      const event = new MessageEvent(type, { data: JSON.stringify(data) });
      for (const listener of this.listeners.get(type) ?? []) listener(event);
    }

    removeEventListener(type: string, listener: EventListenerOrEventListenerObject): void {
      this.listeners.get(type)?.delete(listener as (event: MessageEvent<string>) => void);
      record('ActiveTranscriptSubscriptions', [...eventSources].filter(source => source.isTranscript && source.subscriptionCount > 0).length);
    }

    close(): void {
      if (this.readyState === FixtureEventSource.CLOSED) return;
      this.readyState = FixtureEventSource.CLOSED;
      eventSources.delete(this);
      record('EventSourceCloses', Number(document.documentElement.dataset['productConversationFixtureEventSourceCloses'] ?? '0') + 1);
      record('EventSourceLastClosedInstance', this.instanceId);
      record('ActiveTranscriptSubscriptions', [...eventSources].filter(source => source.isTranscript && source.subscriptionCount > 0).length);
    }
  }
  let holdOpening = false;
  const holdSuccessorOpening = () => { holdOpening = true; };
  const acceptSuccessorOpening = () => {
    if (!currentSnapshot || currentSnapshot.latest_opening.status !== 'pending') return;
    const pending = currentSnapshot.latest_opening;
    currentSnapshot = { ...currentSnapshot, latest_opening: {
      status: 'accepted', edge: pending.edge, accepted_message_id: 'fixture-successor-opening',
      opening_authority: pending.opening_authority, presentation: 'boundary',
    } };
    window.dispatchEvent(new Event('focus'));
  };
  window.addEventListener('product-conversation-fixture-hold-opening', holdSuccessorOpening);
  window.addEventListener('product-conversation-fixture-accept-opening', acceptSuccessorOpening);
  const publishContinuation = () => {
    if (!scenario.continuationJourney || continuationPublished || !currentSnapshot) return;
    continuationPublished = true;
    const previous = currentConversation;
    const predecessorId = previous.id;
    const successorId = 'fixture-successor';
    const last = currentSnapshot.segments.at(-1);
    if (!last) throw new Error('Continuation fixture requires an existing transcript member');
    const boundaryId = 'fixture-continuation-boundary';
    const summary = 'The mobile investigation continues in the next transcript member.';
    const predecessorSegment = { ...last, messages: [...last.messages, {
      ...last.messages.at(-1)!, message_id: boundaryId, conversation_id: predecessorId,
      sequence_id: Math.max(...last.messages.map(message => message.sequence_id), 1) + 1,
      message_type: 'continuation' as const, content: { summary },
    }] };
    const successorSegment = { ...last, segment_ordinal: last.segment_ordinal + 1, transcript_row_id: successorId, slug: successorId,
      title: 'Continued work', messages: [] };
    currentSnapshot = { ...currentSnapshot, latest_transcript_row_id: successorId, writable_transcript_row_id: successorId,
      canonical_route: scenario.kind === 'coordinator' ? `/global/${successorId}` : `/product-conversations/${currentSnapshot.product_conversation_id}`,
      boundaries: [...currentSnapshot.boundaries, { message_id: boundaryId, represented_successor_opening_message_id: null,
        predecessor_transcript_row_id: predecessorId, successor_transcript_row_id: successorId, segment_ordinal: last.segment_ordinal }],
      segments: [...currentSnapshot.segments.slice(0, -1), predecessorSegment, successorSegment],
      latest_opening: holdOpening ? {
        status: 'pending', client_turn_key: 'fixture-successor-opening', opening_authority: 'generated_predecessor_context',
        edge: { product_conversation_id: currentSnapshot.product_conversation_id,
          predecessor_transcript_row_id: predecessorId, successor_transcript_row_id: successorId, continuation_message_id: boundaryId },
      } : { status: 'none' } };
    predecessorConversation = { ...previous, continued_in_conv_id: successorId };
    currentConversation = { ...previous, id: successorId, slug: successorId, state: { type: 'idle' }, continued_in_conv_id: null, message_count: 0 };
    route = { id: successorId, slug: successorId };
    currentMessages = [];
    record('ContinuationSnapshotPersisted', successorId);
    record('ContinuationOldSubscription', latestEventSource?.id ?? 0);
    const predecessorSource = [...eventSources].find(source => source.memberId === predecessorId && source.subscriptionCount > 0);
    if (!predecessorSource) throw new Error('Predecessor SSE is not subscribed');
    predecessorSource.emit('conversation_update', { sequence_id: predecessorSource.nextSequenceId(), conversation: { id: predecessorId, continued_in_conv_id: successorId } });
    record('ContinuationUpdateInstance', predecessorSource.id);
  };
  window.addEventListener('product-conversation-fixture-continue', publishContinuation);
  const publishPhase = (event: Event) => {
    const phase = (event as CustomEvent<{ type: string; attempt?: number; summary?: string }>).detail;
    const source = latestEventSource;
    if (!phase || !source || source.readyState !== FixtureEventSource.OPEN || source.subscriptionCount < 1) return;
    source.emit('state_change', {
      sequence_id: source.nextSequenceId(),
      state: phase,
      presentation_mode: phase.type,
      state_updated_at: '2026-07-01T12:00:01Z',
    });
  };
  window.addEventListener('product-conversation-fixture-phase', publishPhase);
  let successorMessageCount = 0;
  const publishSuccessorMessage = () => {
    if (!scenario.continuationJourney || !continuationPublished) return;
    const successor = [...eventSources].find(source => source.memberId === currentConversation.id && source.readyState === FixtureEventSource.OPEN && source.subscriptionCount > 0);
    if (!successor) throw new Error('Successor SSE has not opened');
    successorMessageCount += 1;
    const message: Message = {
      message_id: `fixture-successor-message-${successorMessageCount}`, conversation_id: currentConversation.id,
      sequence_id: successor.nextSequenceId(), message_type: 'agent', origin: { kind: 'unknown_historical' },
      content: [{ type: 'text', text: successorMessageCount === 1 ? 'Successor is streaming after the handoff.' : 'Successor is streaming after reconnect.' }],
      display_data: null, usage_data: null, event_presentation: null, created_at: '2026-07-01T12:01:00Z',
    };
    currentMessages = [...currentMessages, message];
    successor.emit('message', { opening: null, sequence_id: message.sequence_id, message });
    record('SuccessorMessageInstance', successor.id);
  };
  window.addEventListener('product-conversation-fixture-successor-message', publishSuccessorMessage);

  globalThis.EventSource = FixtureEventSource as unknown as typeof EventSource;
  const failLatestEventSource = () => {
    const stream = [...eventSources].find(source => source.memberId === currentConversation.id && source.readyState === FixtureEventSource.OPEN && source.subscriptionCount > 0);
    if (!stream) throw new Error('Current transcript SSE has not opened');
    record('FailedTranscriptInstance', stream.id);
    stream.triggerNativeError();
  };
  const acknowledgeLastSend = () => { if (lastSubmittedMessage) latestEventSource?.emitMessage(lastSubmittedMessage); };
  window.addEventListener('product-conversation-fixture-acknowledge-last-send', acknowledgeLastSend);
  window.addEventListener('product-conversation-fixture-stream-failure', failLatestEventSource);

  return () => {
    window.removeEventListener('product-conversation-fixture-hold-older', holdOlderPage);
    window.removeEventListener('product-conversation-fixture-stale-snapshot', holdStaleSnapshot);
    window.removeEventListener('product-conversation-fixture-fail-snapshot', failSnapshot);
    window.removeEventListener('product-conversation-fixture-restore-snapshot', restoreSnapshot);
    window.removeEventListener('product-conversation-fixture-release-older', releaseOlderPage);
    window.removeEventListener('product-conversation-fixture-hold-successor', holdSuccessor);
    window.removeEventListener('product-conversation-fixture-release-successor', releaseSuccessor);
    releaseSuccessor();
    window.removeEventListener('product-conversation-fixture-hold-successor-readback', holdSuccessorConversationRead);
    window.removeEventListener('product-conversation-fixture-release-successor-readback', releaseSuccessorConversationRead);
    releaseSuccessorConversationRead();
    window.removeEventListener('product-conversation-fixture-complete-upload', completeUpload);
    window.removeEventListener('product-conversation-fixture-hold-upload', holdFixtureUpload);
    api.uploadAttachments = originalUploadAttachments;
    releaseOlderPage();
    window.removeEventListener('product-conversation-fixture-stream-failure', failLatestEventSource);
    window.removeEventListener('product-conversation-fixture-acknowledge-last-send', acknowledgeLastSend);
    window.removeEventListener('product-conversation-fixture-continue', publishContinuation);
    window.removeEventListener('product-conversation-fixture-hold-opening', holdSuccessorOpening);
    window.removeEventListener('product-conversation-fixture-accept-opening', acceptSuccessorOpening);
    window.removeEventListener('product-conversation-fixture-phase', publishPhase);
    window.removeEventListener('product-conversation-fixture-successor-message', publishSuccessorMessage);
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
