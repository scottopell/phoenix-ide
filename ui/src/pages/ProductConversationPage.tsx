import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { useLocation, useParams } from 'react-router-dom';
import { ConversationNavStack } from '../components/ConversationNavStack';
import { MessageListSkeleton } from '../components/Skeleton';
import {
  api,
  streamApi,
  type ChainQaRow,
  type ChainSseEventData,
  type ChainView,
  type ConversationState,
  type Message,
  type ProductConversationSnapshotView,
} from '../api';
import { ChainProvider, useChainAtom, type InflightQa } from '../chain';
import { ChainWorkIdentityBlock } from '../components/ChainWorkIdentityBlock';
import { parseConversationState } from '../utils';
import type { EnrichedMessage } from '../generated/EnrichedMessage';
import { ChainQaColumn, ChainWorkScopeDock } from './ChainPage';
import { EmbeddedConversationPage, type EmbeddedConversationProjection } from './ConversationPage';
import './ProductConversationPage.css';

const PAGE_SIZE = 100;

type OwnedSnapshot = {
  productConversationId: string;
  value: ProductConversationSnapshotView;
};

function toMessage(message: EnrichedMessage): Message {
  return {
    message_id: message.message_id,
    conversation_id: message.conversation_id,
    sequence_id: message.sequence_id,
    message_type: message.message_type,
    content: message.content as Message['content'],
    display_data: (message.display_data ?? null) as Exclude<Message['display_data'], undefined>,
    usage_data: message.usage_data,
    created_at: message.created_at,
  };
}

function sortSegments(snapshot: ProductConversationSnapshotView) {
  return snapshot.segments
    .slice()
    .sort((a, b) => a.segment_ordinal - b.segment_ordinal);
}

function makeHandoffMessage(snapshot: ProductConversationSnapshotView, segmentOrdinal: number): Message | null {
  const segment = sortSegments(snapshot).find((candidate) => candidate.segment_ordinal === segmentOrdinal);
  if (!segment?.handoff) return null;
  return {
    message_id: `product-handoff:${snapshot.product_conversation_id}:${segment.transcript_row_id}:${segment.handoff.continuation_message_id}`,
    conversation_id: snapshot.product_conversation_id,
    sequence_id: Number.MIN_SAFE_INTEGER + segment.segment_ordinal,
    message_type: 'system',
    content: { text: segment.handoff.summary },
    display_data: {
      productHistoricalHandoff: {
        predecessor_transcript_row_id: segment.handoff.predecessor_transcript_row_id,
        successor_transcript_row_id: segment.handoff.successor_transcript_row_id,
        continuation_message_id: segment.handoff.continuation_message_id,
        segment_ordinal: segment.segment_ordinal,
      },
    },
    created_at: segment.messages[0]?.created_at ?? snapshot.updated_at,
  };
}

function isLatestSegment(snapshot: ProductConversationSnapshotView, transcriptRowId: string): boolean {
  return transcriptRowId === snapshot.latest_transcript_row_id;
}

function flattenHistoricalMessages(snapshot: ProductConversationSnapshotView): Message[] {
  return sortSegments(snapshot)
    .flatMap((segment) => {
      if (isLatestSegment(snapshot, segment.transcript_row_id)) return [];
      const handoffMessage = makeHandoffMessage(snapshot, segment.segment_ordinal);
      const segmentMessages = segment.messages.map(toMessage);
      return handoffMessage ? [...segmentMessages, handoffMessage] : segmentMessages;
    });
}

function mergeMessagesById(snapshotMessages: Message[], liveMessages: Message[]): Message[] {
  const liveById = new Map(liveMessages.map((message) => [message.message_id, message]));
  const merged = snapshotMessages.map((message) => liveById.get(message.message_id) ?? message);
  const snapshotIds = new Set(snapshotMessages.map((message) => message.message_id));
  return [...merged, ...liveMessages.filter((message) => !snapshotIds.has(message.message_id))];
}

function makeAggregateMessages(
  snapshot: ProductConversationSnapshotView,
  latestProjection: EmbeddedConversationProjection | null,
): Message[] {
  const historical = flattenHistoricalMessages(snapshot);
  if (!latestProjection) {
    const latestSegment = sortSegments(snapshot).find((segment) => isLatestSegment(snapshot, segment.transcript_row_id));
    const latestMessages = latestSegment
      ? [
        ...latestSegment.messages.map(toMessage),
        ...(latestSegment.handoff
          ? [makeHandoffMessage(snapshot, latestSegment.segment_ordinal)!]
          : []),
      ]
      : [];
    return [...historical, ...latestMessages];
  }
  const latestSegment = sortSegments(snapshot).find((segment) => isLatestSegment(snapshot, segment.transcript_row_id));
  const latestHandoff = latestSegment ? makeHandoffMessage(snapshot, latestSegment.segment_ordinal) : null;
  const latestSnapshotMessages = latestSegment?.messages.map(toMessage) ?? [];
  return [
    ...historical,
    ...mergeMessagesById(latestSnapshotMessages, latestProjection.messages),
    ...(latestHandoff ? [latestHandoff] : []),
  ];
}

function mergeOlderSegments(
  current: ProductConversationSnapshotView,
  older: ProductConversationSnapshotView,
): ProductConversationSnapshotView {
  const mergedByRowId = new Map(current.segments.map((segment) => [segment.transcript_row_id, segment]));

  for (const olderSegment of older.segments) {
    const currentSegment = mergedByRowId.get(olderSegment.transcript_row_id);
    if (!currentSegment) {
      mergedByRowId.set(olderSegment.transcript_row_id, olderSegment);
      continue;
    }

    const seenMessageIds = new Set(currentSegment.messages.map((message) => message.message_id));
    const prependedOlderMessages = olderSegment.messages.filter((message) => !seenMessageIds.has(message.message_id));
    mergedByRowId.set(olderSegment.transcript_row_id, {
      ...currentSegment,
      ...olderSegment,
      messages: [...prependedOlderMessages, ...currentSegment.messages],
      handoff: olderSegment.handoff ?? currentSegment.handoff,
    });
  }

  return {
    ...current,
    ...older,
    segments: Array.from(mergedByRowId.values()).sort((a, b) => a.segment_ordinal - b.segment_ordinal),
    before: older.before,
    has_older: older.has_older,
  };
}

function aggregateConversationState(snapshot: ProductConversationSnapshotView, loadingOlder: boolean, olderError: string | null): ConversationState {
  if (olderError) {
    return { type: 'error', message: olderError, error_kind: 'server_error' };
  }
  if (loadingOlder) {
    return { type: 'awaiting_llm' };
  }
  const latestSegment = snapshot.segments
    .slice()
    .sort((a, b) => a.segment_ordinal - b.segment_ordinal)
    .findLast((segment) => segment.transcript_row_id === snapshot.writable_transcript_row_id)
    ?? snapshot.segments[snapshot.segments.length - 1]
    ?? null;
  const lastMessage = latestSegment?.messages[latestSegment.messages.length - 1] as unknown as Record<string, unknown> | undefined;
  const maybeState = lastMessage?.['display_data'] && typeof lastMessage['display_data'] === 'object'
    ? (lastMessage['display_data'] as Record<string, unknown>)['conversation_state']
    : undefined;
  return parseConversationState(maybeState);
}

function chainFromSnapshot(snapshot: ProductConversationSnapshotView, qaHistory: ChainQaRow[]): ChainView {
  return {
    root_conv_id: snapshot.chain_qa_compatibility?.root_transcript_row_id ?? snapshot.requested_transcript_row_id,
    chain_name: null,
    display_name: snapshot.presentation.display_name,
    archived: snapshot.ordinary_lifecycle === 'history',
    members: snapshot.segments
      .slice()
      .sort((a, b) => a.segment_ordinal - b.segment_ordinal)
      .map((segment, index, segments) => ({
        conv_id: segment.transcript_row_id,
        slug: segment.slug,
        title: segment.title,
        updated_at: snapshot.updated_at,
        message_count: segment.messages.length,
        has_worktree: snapshot.work_identity?.work_transcript_row_id === segment.transcript_row_id,
        position: index === 0 ? 'root' : index === segments.length - 1 ? 'latest' : 'continuation',
      })),
    qa_history: qaHistory,
    current_member_count: snapshot.segments.length,
    current_total_messages: snapshot.segments.reduce((sum, segment) => sum + segment.messages.length, 0),
    work_identity: snapshot.work_identity ? {
      work_conv_id: snapshot.work_identity.work_transcript_row_id,
      branch_name: snapshot.work_identity.branch_name,
      base_branch: snapshot.work_identity.base_branch,
      worktree_path: snapshot.work_identity.worktree_path,
      task_id: snapshot.work_identity.task_id,
      task_title: snapshot.work_identity.task_title,
    } : null,
  };
}

function SourceMeta({ snapshot }: { snapshot: ProductConversationSnapshotView }) {
  return (
    <section className="product-conversation-meta" aria-label="Product conversation metadata">
      <div className="product-conversation-meta__row">
        <span className="product-conversation-meta__label">Presentation</span>
        <span>{snapshot.presentation.display_name}</span>
      </div>
      <div className="product-conversation-meta__row">
        <span className="product-conversation-meta__label">Lifecycle</span>
        <span>{snapshot.ordinary_lifecycle}</span>
      </div>
      <div className="product-conversation-meta__row">
        <span className="product-conversation-meta__label">Canonical root</span>
        <span>{snapshot.canonical_root.title ?? snapshot.canonical_root.slug ?? snapshot.canonical_root.transcript_row_id}</span>
      </div>
      {snapshot.source && (
        <div className="product-conversation-meta__row">
          <span className="product-conversation-meta__label">Source</span>
          <span>
            {snapshot.source.relation} · {snapshot.source.status} · {snapshot.source.relation_key}
          </span>
        </div>
      )}
    </section>
  );
}


export function ProductConversationPage() {
  return (
    <ChainProvider>
      <ProductConversationPageInner />
    </ChainProvider>
  );
}

function ProductConversationPageInner() {
  const { productConversationId } = useParams<{ productConversationId: string }>();
  const location = useLocation();
  const hashTargetMessageId = location.hash.startsWith('#message-')
    ? decodeURIComponent(location.hash.slice('#message-'.length))
    : null;
  const [ownedSnapshot, setOwnedSnapshot] = useState<OwnedSnapshot | null>(null);
  const snapshot = ownedSnapshot && ownedSnapshot.productConversationId === productConversationId
    ? ownedSnapshot.value
    : null;
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [olderError, setOlderError] = useState<string | null>(null);
  const [latestProjection, setLatestProjection] = useState<EmbeddedConversationProjection | null>(null);
  const [historyGeneration, setHistoryGeneration] = useState(0);
  const chainRootId = snapshot?.chain_qa_compatibility?.root_transcript_row_id ?? null;
  const [atom, dispatch] = useChainAtom(chainRootId);
  const { chain, inflight, inflightOrder, draft, submitting, sseLost, loadError } = atom;
  const activeTextareaRef = useRef<HTMLTextAreaElement | null>(null);
  const inflightRef = useRef(inflight);
  inflightRef.current = inflight;

  useEffect(() => {
    setLatestProjection(null);
  }, [productConversationId]);

  useEffect(() => {
    if (!productConversationId) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    setOlderError(null);

    api.getProductConversationSnapshot(productConversationId, { message_limit: PAGE_SIZE })
      .then((next) => {
        if (cancelled) return;
        setOwnedSnapshot({ productConversationId, value: next });
        setHistoryGeneration(0);
      })
      .catch((err) => {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : 'Unable to open this product conversation.');
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => { cancelled = true; };
  }, [productConversationId]);

  const refreshChain = useCallback(async (rootId: string) => {
    try {
      const view = await api.getChain(rootId);
      dispatch({ type: 'LOAD_OK', view });
      return view;
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to load chain';
      dispatch({ type: 'LOAD_FAIL', error: msg });
      return null;
    }
  }, [dispatch]);

  useEffect(() => {
    if (!chainRootId) return;
    dispatch({ type: 'LOAD_BEGIN' });
    void refreshChain(chainRootId);
  }, [chainRootId, dispatch, refreshChain]);

  useEffect(() => {
    if (!chainRootId) return;
    const handleEvent = (evt: ChainSseEventData) => {
      if (evt.type === 'chain_qa_token') {
        dispatch({ type: 'TOKEN_APPENDED', chainQaId: evt.chain_qa_id, delta: evt.delta });
      } else if (evt.type === 'chain_qa_completed') {
        dispatch({ type: 'INFLIGHT_DROP', chainQaId: evt.chain_qa_id });
        void refreshChain(chainRootId);
      } else if (evt.type === 'chain_qa_failed') {
        dispatch({
          type: 'INFLIGHT_FAIL',
          chainQaId: evt.chain_qa_id,
          error: evt.error,
          partialAnswer: evt.partial_answer ?? null,
        });
        void refreshChain(chainRootId).then(() => {
          dispatch({ type: 'INFLIGHT_DROP', chainQaId: evt.chain_qa_id });
        });
      }
    };
    const handleErr = () => {
      if (Object.keys(inflightRef.current).length > 0) {
        dispatch({ type: 'SSE_LOST' });
      }
    };
    const es = streamApi.subscribeToChainStream(chainRootId, handleEvent, handleErr);
    dispatch({ type: 'SSE_RESTORED' });
    return () => es.close();
  }, [chainRootId, dispatch, refreshChain]);

  const loadOlderMessages = useCallback(async () => {
    if (!productConversationId || !snapshot?.has_older || !snapshot.before || loadingOlder) return;
    setLoadingOlder(true);
    setOlderError(null);
    try {
      const older = await api.getProductConversationSnapshot(productConversationId, {
        message_limit: PAGE_SIZE,
        before: snapshot.before,
      });
      setOwnedSnapshot((current) => {
        if (!current || current.productConversationId !== productConversationId) {
          return { productConversationId, value: older };
        }
        return { productConversationId, value: mergeOlderSegments(current.value, older) };
      });
      setHistoryGeneration((generation) => generation + 1);
    } catch (err) {
      setOlderError(err instanceof Error ? err.message : 'Failed to load earlier history.');
    } finally {
      setLoadingOlder(false);
    }
  }, [loadingOlder, productConversationId, snapshot]);

  const messages = useMemo(
    () => snapshot ? makeAggregateMessages(snapshot, latestProjection) : [],
    [latestProjection, snapshot],
  );
  const convState = useMemo(
    () => latestProjection?.convState ?? (snapshot ? aggregateConversationState(snapshot, loadingOlder, olderError) : { type: 'idle' } satisfies ConversationState),
    [latestProjection, loadingOlder, olderError, snapshot],
  );
  const latestSlug = latestProjection?.slug ?? snapshot?.latest_transcript_row_id ?? null;
  const latestConversationId = latestProjection?.conversationId;
  const latestWorkScopeKey = latestProjection?.isArchived ? undefined : latestProjection?.conversation?.work_scope_key;
  const transcriptView = {
    conversationId: latestConversationId ?? snapshot?.product_conversation_id ?? '',
    generation: historyGeneration,
    transcriptGeneration: historyGeneration,
  };
  const hashTargetLoaded = !!hashTargetMessageId && messages.some((message) => message.message_id === hashTargetMessageId);
  const transcriptPositioning = hashTargetLoaded
    ? {
      kind: 'positioning' as const,
      command: {
        kind: 'jump_to_message' as const,
        token: 1,
        requestToken: 0,
        view: transcriptView,
        targetMessageId: hashTargetMessageId,
      },
    }
    : { kind: 'idle' as const, view: transcriptView };
  const isOpen = snapshot?.ordinary_lifecycle === 'open';

  useEffect(() => {
    if (!hashTargetMessageId || hashTargetLoaded || !snapshot?.has_older || loadingOlder || olderError) return;
    void loadOlderMessages();
  }, [hashTargetLoaded, hashTargetMessageId, loadOlderMessages, loadingOlder, olderError, snapshot?.has_older]);

  const synthChain = useMemo(() => {
    if (!snapshot) return null;
    return chain ?? chainFromSnapshot(snapshot, []);
  }, [chain, snapshot]);

  const renderableQas = useMemo(() => {
    const persisted: ChainQaRow[] = chain?.qa_history.slice() ?? [];
    persisted.sort((a, b) => (a.created_at < b.created_at ? -1 : 1));
    const inflightList: InflightQa[] = inflightOrder
      .map((id) => inflight[id])
      .filter((entry): entry is InflightQa => entry !== undefined);
    return { persisted, inflightList };
  }, [chain, inflight, inflightOrder]);

  const submit = useCallback(async (question: string) => {
    if (!chainRootId) return;
    const trimmed = question.trim();
    if (!trimmed) return;
    const tempId = `temp-${crypto.randomUUID()}`;
    dispatch({ type: 'OPTIMISTIC_INFLIGHT_ADD', chainQaId: tempId, question: trimmed });
    dispatch({ type: 'SUBMIT_BEGIN' });
    queueMicrotask(() => activeTextareaRef.current?.focus());
    try {
      const { chain_qa_id } = await api.submitChainQuestion(chainRootId, trimmed);
      dispatch({ type: 'INFLIGHT_RECONCILE_ID', tempId, realId: chain_qa_id });
      dispatch({ type: 'SUBMIT_OK' });
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to submit question';
      dispatch({ type: 'INFLIGHT_DROP', chainQaId: tempId });
      dispatch({ type: 'DRAFT_SET', value: trimmed });
      dispatch({ type: 'SUBMIT_FAIL', error: msg });
    }
  }, [chainRootId, dispatch]);

  const handleSubmit = (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    void submit(draft);
  };

  const draftRef = useRef(draft);
  useEffect(() => {
    draftRef.current = draft;
  }, [draft]);

  const handleReask = useCallback((question: string) => {
    dispatch({ type: 'DRAFT_SET', value: draftRef.current.trim() ? `${draftRef.current}\n\n${question}` : question });
    queueMicrotask(() => activeTextareaRef.current?.focus());
  }, [dispatch]);

  const setDraft = useCallback((value: string) => dispatch({ type: 'DRAFT_CHANGED', value }), [dispatch]);

  if (loading) {
    return (
      <main className="product-conversation-page">
        <header className="product-conversation-page__header">
          <h1>Product conversation</h1>
        </header>
        <MessageListSkeleton count={6} />
      </main>
    );
  }

  if (error || !snapshot) {
    return <main className="product-conversation-page" role="alert">{error ?? 'Unable to open this product conversation.'}</main>;
  }

  return (
    <main className="product-conversation-page" data-testid="product-conversation-page">
      <header className="product-conversation-page__header">
        <div>
          <div className="product-conversation-page__eyebrow">Product conversation</div>
          <h1>{snapshot.presentation.display_name}</h1>
          <p className="product-conversation-page__route">{snapshot.canonical_route}</p>
        </div>
        <div className="product-conversation-page__status">
          {loadingOlder ? 'Loading earlier history…' : olderError ? olderError : snapshot.has_older ? 'Earlier history available' : 'Complete snapshot loaded'}
        </div>
      </header>

      <div className="product-conversation-page__layout">
        <section className="product-conversation-page__history">
          <SourceMeta snapshot={snapshot} />
          {synthChain?.work_identity && <ChainWorkIdentityBlock identity={synthChain.work_identity} />}
          <ConversationNavStack
            messages={messages}
            pendingMessages={latestProjection?.pendingMessages ?? []}
            convState={convState}
            onRetry={latestProjection?.onRetryPending ?? (() => {})}
            onCancelSteering={latestProjection?.onCancelSteering}
            onOpenFile={latestProjection?.onOpenFile}
            enableMessageSidepanel={false}
            enableMessageFullscreen={false}
            conversationId={latestConversationId ?? snapshot.product_conversation_id}
            slug={latestSlug ?? snapshot.canonical_root.slug ?? snapshot.requested_transcript_row_id}
            hasOlderMessages={snapshot.has_older}
            onLoadOlderMessages={snapshot.has_older ? () => { void loadOlderMessages(); } : undefined}
            loadingOlderMessages={loadingOlder}
            olderHistoryError={olderError}
            transcriptPositioning={transcriptPositioning}
            {...(latestWorkScopeKey ? { workScopeKey: latestWorkScopeKey } : {})}
          />
        </section>

        {(chainRootId || (isOpen && snapshot.latest_transcript_row_id)) && (
          <section className="product-conversation-page__qa" aria-label="Product conversation recall and live controls">
            {loadError && (
              <div role="alert" className="product-conversation-page__error">
                <span>Failed to load recall history: {loadError}</span>
                <button type="button" onClick={() => { if (chainRootId) void refreshChain(chainRootId); }}>Retry</button>
              </div>
            )}
            {synthChain && chainRootId && (
              <ChainQaColumn
                chain={synthChain}
                persisted={renderableQas.persisted}
                inflight={renderableQas.inflightList}
                draft={draft}
                setDraft={setDraft}
                submitting={submitting}
                sseLost={sseLost}
                onSubmit={loadError ? () => {} : handleSubmit}
                onReask={handleReask}
                activeTextareaRef={activeTextareaRef}
                onRetryConnection={() => {
                  dispatch({ type: 'SSE_RESTORED' });
                  void refreshChain(chainRootId);
                }}
              />
            )}
            {isOpen && snapshot.latest_transcript_row_id ? (
              <div className="product-conversation-page__composer" data-testid="product-conversation-composer">
                <EmbeddedConversationPage
                  slug={snapshot.latest_transcript_row_id}
                  showTranscript={false}
                  suppressCanonicalization={true}
                  ordinaryComposerEnabled={snapshot.writable_transcript_row_id === snapshot.latest_transcript_row_id}
                  onProjectionChange={setLatestProjection}
                />
              </div>
            ) : (
              <div className="product-conversation-page__composer-placeholder">History is read-only.</div>
            )}
          </section>
        )}

        {snapshot.latest_transcript_row_id && synthChain?.work_identity && (
          <aside className="product-conversation-page__dock">
            <ChainWorkScopeDock
              activeConvId={snapshot.latest_transcript_row_id}
              workIdentity={synthChain.work_identity}
            />
          </aside>
        )}
      </div>
    </main>
  );
}
