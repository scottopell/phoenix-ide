import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { useLocation, useParams } from 'react-router-dom';
import { ConversationNavStack } from '../components/ConversationNavStack';
import { ChainWorkIdentityBlock } from '../components/ChainWorkIdentityBlock';
import { MessageListSkeleton } from '../components/Skeleton';
import {
  ApiResponseError,
  api,
  streamApi,
  type ChainQaRow,
  type ChainSseEventData,
  type ChainView,
  type ConversationState,
  type Message,
  type ProductConversationSnapshotView,
} from '../api';
import { useChainAtom, type InflightQa } from '../chain';
import { parseConversationState } from '../utils';
import type { EnrichedMessage } from '../generated/EnrichedMessage';
import type { RestoreBasis } from '../conversation/historyExpansion';
import type { TranscriptPositioningInput } from '../conversation/transcriptPositioning';
import { OPEN_MESSAGE_VIEWER_EVENT, type OpenMessageViewerEventDetail } from '../components/MessageContextMenu';
import { useViewerSlot } from '../contexts/ViewerSlotContext';
import { ReviewNotesProvider } from '../contexts/ReviewNotesContext';
import { useIsWideDesktop } from '../hooks/useMediaQuery';
import { EmbeddedConversationPage, type EmbeddedConversationProjection } from './ConversationPage';
import { subscribeCloseSnapshotChanged } from '../notifications';
import './ProductConversationPage.css';

const PAGE_SIZE = 100;
const MessageViewer = lazy(() =>
  import('../components/MessageViewer').then((m) => ({ default: m.MessageViewer })),
);
const TaskApprovalReader = lazy(() =>
  import('../components/TaskApprovalReader').then((m) => ({ default: m.TaskApprovalReader })),
);
const FirstTaskWelcome = lazy(() =>
  import('../components/FirstTaskWelcome').then((m) => ({ default: m.FirstTaskWelcome })),
);
const ChainQaColumn = lazy(() =>
  import('./ChainPage').then((m) => ({ default: m.ChainQaColumn })),
);


async function fetchOlderSnapshotWithFreshCursor(
  productConversationId: string,
  before: string,
): Promise<{ older: ProductConversationSnapshotView; refreshedTail?: ProductConversationSnapshotView }> {
  try {
    return { older: await api.getProductConversationSnapshot(productConversationId, {
      message_limit: PAGE_SIZE,
      before,
    }) };
  } catch (error) {
    if (!(error instanceof ApiResponseError) || error.status !== 400) throw error;
    const refreshed = await api.getProductConversationSnapshot(productConversationId, { message_limit: PAGE_SIZE });
    if (!refreshed.has_older || !refreshed.before) throw error;
    return {
      refreshedTail: refreshed,
      older: await api.getProductConversationSnapshot(productConversationId, {
        message_limit: PAGE_SIZE,
        before: refreshed.before,
      }),
    };
  }
}

type OwnedSnapshot = {
  productConversationId: string;
  value: ProductConversationSnapshotView;
};

type TaskApprovalOverlayState = {
  title: string;
  priority: 'p0' | 'p1' | 'p2' | 'p3' | 'p4';
  plan: string;
  conversationId: string;
};

function productOccurrenceToken(segmentTranscriptRowId: string, messageId: string): string {
  return `${segmentTranscriptRowId}:${messageId}`;
}

function decodeMessageHash(hash: string): string | null {
  if (!hash.startsWith('#message-')) return null;
  try {
    return decodeURIComponent(hash.slice('#message-'.length));
  } catch {
    return null;
  }
}

function toMessage(message: EnrichedMessage, occurrenceToken?: string): Message {
  return {
    message_id: message.message_id,
    conversation_id: message.conversation_id,
    sequence_id: message.sequence_id,
    message_type: message.message_type,
    content: message.content as Message['content'],
    display_data: occurrenceToken
      ? {
        ...(((message.display_data ?? null) as Exclude<Message['display_data'], undefined>) ?? {}),
        productOccurrenceToken: occurrenceToken,
      }
      : (message.display_data ?? null) as Exclude<Message['display_data'], undefined>,
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
      const segmentMessages = segment.messages.map((message) => toMessage(message, productOccurrenceToken(segment.transcript_row_id, message.message_id)));
      return handoffMessage ? [...segmentMessages, handoffMessage] : segmentMessages;
    });
}

function mergeMessagesById(snapshotMessages: Message[], liveMessages: Message[], transcriptRowId: string): Message[] {
  const liveById = new Map(liveMessages.map((message) => [message.message_id, message]));
  const merged = snapshotMessages.map((message) => {
    const live = liveById.get(message.message_id);
    if (!live) return message;
    return {
      ...live,
      display_data: {
        ...(live.display_data ?? {}),
        ...((message.display_data as { productOccurrenceToken?: string } | null | undefined)?.productOccurrenceToken
          ? { productOccurrenceToken: (message.display_data as { productOccurrenceToken: string }).productOccurrenceToken }
          : {}),
      },
    };
  });
  const snapshotIds = new Set(snapshotMessages.map((message) => message.message_id));
  const appended = liveMessages
    .filter((message) => !snapshotIds.has(message.message_id))
    .map((message) => ({
      ...message,
      display_data: {
        ...(message.display_data ?? {}),
        productOccurrenceToken: productOccurrenceToken(transcriptRowId, message.message_id),
      },
    }));
  return [...merged, ...appended];
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
        ...latestSegment.messages.map((message) => toMessage(message, productOccurrenceToken(latestSegment.transcript_row_id, message.message_id))),
        ...(latestSegment.handoff
          ? [makeHandoffMessage(snapshot, latestSegment.segment_ordinal)!]
          : []),
      ]
      : [];
    return [...historical, ...latestMessages];
  }
  const latestSegment = sortSegments(snapshot).find((segment) => isLatestSegment(snapshot, segment.transcript_row_id));
  const latestHandoff = latestSegment ? makeHandoffMessage(snapshot, latestSegment.segment_ordinal) : null;
  const latestSnapshotMessages = latestSegment?.messages.map((message) => toMessage(message, productOccurrenceToken(latestSegment.transcript_row_id, message.message_id))) ?? [];
  return [
    ...historical,
    ...mergeMessagesById(latestSnapshotMessages, latestProjection.messages, snapshot.latest_transcript_row_id),
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

function mergeRefreshedTail(
  current: ProductConversationSnapshotView,
  refreshed: ProductConversationSnapshotView,
): ProductConversationSnapshotView {
  const segmentsByRowId = new Map(
    refreshed.segments.map((segment) => [segment.transcript_row_id, segment]),
  );
  const earliestRefreshedOrdinal = refreshed.segments.length > 0
    ? Math.min(...refreshed.segments.map((segment) => segment.segment_ordinal))
    : null;
  for (const retainedSegment of current.segments) {
    const refreshedSegment = segmentsByRowId.get(retainedSegment.transcript_row_id);
    if (!refreshedSegment) {
      if (earliestRefreshedOrdinal != null
        && retainedSegment.segment_ordinal < earliestRefreshedOrdinal) {
        segmentsByRowId.set(retainedSegment.transcript_row_id, retainedSegment);
      }
      continue;
    }
    const refreshedMessageIds = new Set(
      refreshedSegment.messages.map((message) => message.message_id),
    );
    segmentsByRowId.set(retainedSegment.transcript_row_id, {
      ...retainedSegment,
      ...refreshedSegment,
      messages: [
        ...retainedSegment.messages.filter(
          (message) => !refreshedMessageIds.has(message.message_id),
        ),
        ...refreshedSegment.messages,
      ],
      handoff: refreshedSegment.handoff ?? retainedSegment.handoff,
    });
  }
  const retainedPrefix = earliestRefreshedOrdinal != null
    && current.segments.some(
      (segment) => segment.segment_ordinal < earliestRefreshedOrdinal,
    );
  return {
    ...refreshed,
    segments: Array.from(segmentsByRowId.values())
      .sort((a, b) => a.segment_ordinal - b.segment_ordinal),
    before: retainedPrefix ? current.before : refreshed.before,
    has_older: retainedPrefix ? current.has_older : refreshed.has_older,
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

function countSnapshotMessages(snapshot: ProductConversationSnapshotView): number {
  return snapshot.segments.reduce((sum, segment) => sum + segment.messages.length, 0);
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
    current_total_messages: countSnapshotMessages(snapshot),
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

function RecallPanel({
  snapshot,
  productConversationId,
  messages,
  disabled,
  onClose,
}: {
  snapshot: ProductConversationSnapshotView;
  productConversationId: string;
  messages: Message[];
  disabled: boolean;
  onClose: () => void;
}) {
  const chainRootId = snapshot.chain_qa_compatibility?.root_transcript_row_id ?? null;
  const [atom, dispatch] = useChainAtom(chainRootId);
  const { chain, inflight, inflightOrder, draft, submitting, sseLost, loadError, submitError } = atom;
  const activeTextareaRef = useRef<HTMLTextAreaElement | null>(null);
  const closeButtonRef = useRef<HTMLButtonElement | null>(null);
  const initialFocusHandledRef = useRef(false);
  const [shouldFocusAction, setShouldFocusAction] = useState(false);
  const inflightRef = useRef(inflight);
  const draftRef = useRef(draft);
  const hydratedDraftForRef = useRef<string | null>(null);
  inflightRef.current = inflight;
  draftRef.current = draft;

  const refreshChain = useCallback(async (rootId: string) => {
    try {
      const view = await api.getChain(rootId);
      dispatch({ type: 'LOAD_OK', view });
      return view;
    } catch (err) {
      dispatch({
        type: 'LOAD_FAIL',
        error: err instanceof Error ? err.message : 'Failed to load chain',
      });
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
    const handleError = () => {
      if (Object.keys(inflightRef.current).length > 0) dispatch({ type: 'SSE_LOST' });
    };
    const eventSource = streamApi.subscribeToChainStream(chainRootId, handleEvent, handleError);
    dispatch({ type: 'SSE_RESTORED' });
    return () => eventSource.close();
  }, [chainRootId, dispatch, refreshChain]);

  useEffect(() => {
    if (!chainRootId) return;
    const owner = `${productConversationId}:${chainRootId}`;
    if (hydratedDraftForRef.current === owner) return;
    hydratedDraftForRef.current = owner;
    try {
      const saved = localStorage.getItem(`phoenix:product-conversation-draft:${productConversationId}`);
      if (saved && !draftRef.current.trim()) dispatch({ type: 'DRAFT_SET', value: saved });
    } catch {
      // Storage is optional; the routed atom still preserves the open-session draft.
    }
  }, [chainRootId, dispatch, productConversationId]);

  useEffect(() => {
    if (!chainRootId) return undefined;
    const key = `phoenix:product-conversation-draft:${productConversationId}`;
    if (draft === '') {
      try { localStorage.removeItem(key); } catch { /* Storage is optional. */ }
      return undefined;
    }
    const timer = window.setTimeout(() => {
      try { localStorage.setItem(key, draft); } catch { /* Storage is optional. */ }
    }, 300);
    return () => window.clearTimeout(timer);
  }, [chainRootId, draft, productConversationId]);

  useEffect(() => {
    if (!chainRootId) return undefined;
    const key = `phoenix:product-conversation-draft:${productConversationId}`;
    return () => {
      try {
        if (draftRef.current === '') localStorage.removeItem(key);
        else localStorage.setItem(key, draftRef.current);
      } catch { /* Storage is optional. */ }
    };
  }, [chainRootId, productConversationId]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopPropagation();
      onClose();
    };
    window.addEventListener('keydown', handleKeyDown, true);
    return () => window.removeEventListener('keydown', handleKeyDown, true);
  }, [onClose]);

  useEffect(() => {
    if (!chain || loadError || initialFocusHandledRef.current) return;
    initialFocusHandledRef.current = true;
    if (disabled) {
      closeButtonRef.current?.focus();
      return;
    }
    setShouldFocusAction(true);
  }, [chain, disabled, loadError]);

  const markInitialFocusHandled = useCallback(() => {
    setShouldFocusAction(false);
  }, []);

  const synthChain = useMemo(() => {
    const fallback = chainFromSnapshot(snapshot, []);
    if (!chain) return fallback;
    const realMessageCount = messages.filter((message) => (
      !(message.display_data as { productHistoricalHandoff?: unknown } | null | undefined)?.productHistoricalHandoff
    )).length;
    return {
      ...chain,
      current_member_count: Math.max(chain.current_member_count, snapshot.segments.length),
      current_total_messages: Math.max(chain.current_total_messages, realMessageCount, countSnapshotMessages(snapshot)),
    };
  }, [chain, messages, snapshot]);

  const renderableQas = useMemo(() => {
    const persisted = chain?.qa_history.slice() ?? [];
    persisted.sort((a, b) => (a.created_at < b.created_at ? -1 : 1));
    const inflightList: InflightQa[] = inflightOrder
      .map((id) => inflight[id])
      .filter((entry): entry is InflightQa => entry !== undefined);
    return { persisted, inflightList };
  }, [chain, inflight, inflightOrder]);

  const submit = useCallback(async (question: string) => {
    if (!chainRootId || !chain) return;
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
      dispatch({ type: 'INFLIGHT_DROP', chainQaId: tempId });
      dispatch({ type: 'DRAFT_SET', value: trimmed });
      dispatch({
        type: 'SUBMIT_FAIL',
        error: err instanceof Error ? err.message : 'Failed to submit question',
      });
    }
  }, [chain, chainRootId, dispatch]);

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    void submit(draft);
  };
  const handleReask = useCallback((question: string) => {
    dispatch({ type: 'DRAFT_SET', value: draftRef.current.trim() ? `${draftRef.current}\n\n${question}` : question });
    queueMicrotask(() => activeTextareaRef.current?.focus());
  }, [dispatch]);
  const setDraft = useCallback((value: string) => dispatch({ type: 'DRAFT_CHANGED', value }), [dispatch]);

  return (
    <section
      id="product-conversation-recall-panel"
      className="product-conversation-page__recall-panel"
      role="dialog"
      aria-label="Recall"
      aria-modal="false"
    >
      <div className="product-conversation-page__recall-heading">
        <div>
          <strong>Recall</strong>
          <span>Ask across this conversation’s full lineage</span>
        </div>
        <button ref={closeButtonRef} type="button" className="product-conversation-page__recall-close" onClick={onClose} aria-label="Close Recall">×</button>
      </div>
      {loadError && (
        <div role="alert" className="product-conversation-page__recall-error">
          <span>Failed to load recall history: {loadError}</span>
          <button type="button" className="btn-link" onClick={() => { void refreshChain(chainRootId!); }}>Retry</button>
        </div>
      )}
      {submitError && (
        <div role="alert" className="product-conversation-page__recall-error">
          <span>Failed to ask Recall: {submitError}</span>
        </div>
      )}
      {!chain && !loadError ? (
        <div className="product-conversation-page__recall-loading" role="status">Loading Recall…</div>
      ) : chain ? (
        <Suspense fallback={<div className="product-conversation-page__recall-loading" role="status">Loading Recall…</div>}>
          <ChainQaColumn
            chain={synthChain}
            persisted={renderableQas.persisted}
            inflight={renderableQas.inflightList}
            draft={draft}
            setDraft={setDraft}
            submitting={submitting}
            sseLost={sseLost}
            {...(!disabled ? { onSubmit: handleSubmit } : {})}
            onReask={handleReask}
            disabled={disabled}
            activeTextareaRef={activeTextareaRef}
            autoFocusActive={shouldFocusAction}
            onActiveTextareaFocused={markInitialFocusHandled}
            onRetryConnection={() => {
              dispatch({ type: 'SSE_RESTORED' });
              void refreshChain(chainRootId!);
            }}
          />
        </Suspense>
      ) : null}
    </section>
  );
}

function RecallDisclosure({
  snapshot,
  productConversationId,
  messages,
  disabled,
}: {
  snapshot: ProductConversationSnapshotView;
  productConversationId: string;
  messages: Message[];
  disabled: boolean;
}) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const close = useCallback(() => {
    setOpen(false);
    triggerRef.current?.focus();
  }, []);

  return (
    <div className="product-conversation-page__recall">
      <button
        ref={triggerRef}
        type="button"
        className="product-conversation-page__recall-trigger"
        aria-expanded={open}
        aria-controls="product-conversation-recall-panel"
        onClick={() => setOpen((value) => !value)}
      >
        Recall
      </button>
      {open && (
        <RecallPanel
          snapshot={snapshot}
          productConversationId={productConversationId}
          messages={messages}
          disabled={disabled}
          onClose={close}
        />
      )}
    </div>
  );
}

function sourceRelationLabel(source: NonNullable<ProductConversationSnapshotView['source']>): string {
  switch (source.relation) {
    case 'approved_task':
      return 'Approved task';
  }
}

function ProductConversationHeader({
  snapshot,
  productConversationId,
  messages,
  recallDisabled,
}: {
  snapshot: ProductConversationSnapshotView;
  productConversationId: string;
  messages: Message[];
  recallDisabled: boolean;
}) {
  const source = snapshot.source;
  return (
    <header className="product-conversation-page__header">
      <h1 className="product-conversation-page__title" title={snapshot.presentation.display_name}>
        {snapshot.presentation.display_name}
      </h1>
      <div className="product-conversation-page__context">
        {source?.status === 'present' && (
          <span className="product-conversation-page__source" data-testid="product-conversation-source">
            {sourceRelationLabel(source)} from{' '}
            <a href={`/product-conversations/${encodeURIComponent(source.source_product_conversation_id)}`}>
              source conversation
            </a>
          </span>
        )}
        {source?.status === 'deleted' && (
          <span className="product-conversation-page__source product-conversation-page__source--unavailable" data-testid="product-conversation-source">
            {sourceRelationLabel(source)} source unavailable or deleted
          </span>
        )}
        {snapshot.chain_qa_compatibility && (
          <RecallDisclosure
            key={productConversationId}
            snapshot={snapshot}
            productConversationId={productConversationId}
            messages={messages}
            disabled={recallDisabled}
          />
        )}
        {snapshot.work_identity && (
          <details className="product-conversation-page__work" data-testid="product-conversation-work">
            <summary>Work</summary>
            <ChainWorkIdentityBlock identity={snapshot.work_identity} title={null} />
          </details>
        )}
      </div>
    </header>
  );
}

export function ProductConversationPage() {
  return <ProductConversationPageInner />;
}

function ProductConversationPageInner() {
  const { productConversationId } = useParams<{ productConversationId: string }>();
  const viewerSlot = useViewerSlot();
  const location = useLocation();
  const hashTargetMessageId = decodeMessageHash(location.hash);
  const [ownedSnapshot, setOwnedSnapshot] = useState<OwnedSnapshot | null>(null);
  const ownedSnapshotRef = useRef<OwnedSnapshot | null>(null);
  const snapshot = ownedSnapshot && ownedSnapshot.productConversationId === productConversationId
    ? ownedSnapshot.value
    : null;
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [snapshotRetry, setSnapshotRetry] = useState(0);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [olderError, setOlderError] = useState<string | null>(null);
  const [latestProjection, setLatestProjection] = useState<EmbeddedConversationProjection | null>(null);
  const [taskApprovalOverlay, setTaskApprovalOverlay] = useState<TaskApprovalOverlayState | null>(null);
  const [approvalContextWindowUsed, setApprovalContextWindowUsed] = useState<number | null>(null);
  const [showFirstTaskWelcome, setShowFirstTaskWelcome] = useState(false);
  const [historyGeneration, setHistoryGeneration] = useState(0);
  const [restoreCommand, setRestoreCommand] = useState<TranscriptPositioningInput | null>(null);
  const routeGenerationRef = useRef(0);
  const paginationRequestRef = useRef(0);
  const observedMemberProjectionRef = useRef<typeof latestProjection>(null);
  const aggregateMessages = useMemo(
    () => snapshot ? makeAggregateMessages(snapshot, latestProjection) : [],
    [latestProjection, snapshot],
  );
  const aggregateMessageSlot = viewerSlot.slot.kind === 'message' ? viewerSlot.slot : null;
  const isWideDesktop = useIsWideDesktop();
  const showSplitPaneViewer = isWideDesktop && aggregateMessageSlot?.presentation === 'pane';
  const closeAggregateMessageViewer = viewerSlot.close;
  const appendAggregateReviewNotes = useCallback((formattedNotes: string) => {
    latestProjection?.appendReviewNotesToComposer?.(formattedNotes);
    if (!(isWideDesktop && aggregateMessageSlot?.presentation === 'fullscreen')) {
      closeAggregateMessageViewer();
    }
  }, [aggregateMessageSlot?.presentation, closeAggregateMessageViewer, isWideDesktop, latestProjection]);
  ownedSnapshotRef.current = ownedSnapshot;

  useEffect(() => {
    routeGenerationRef.current += 1;
    paginationRequestRef.current += 1;
    setLatestProjection(null);
    setRestoreCommand(null);
    setOlderError(null);
    setLoadingOlder(false);
    observedMemberProjectionRef.current = null;
  }, [productConversationId]);

  useEffect(() => {
    if (!productConversationId) return;
    let cancelled = false;
    const isBackgroundRefresh = ownedSnapshotRef.current?.productConversationId === productConversationId;
    if (!isBackgroundRefresh) setLoading(true);
    setError(null);
    setOlderError(null);

    api.getProductConversationSnapshot(productConversationId, { message_limit: PAGE_SIZE })
      .then((next) => {
        if (cancelled) return;
        setOwnedSnapshot((current) => ({
          productConversationId,
          value: current?.productConversationId === productConversationId
            ? mergeRefreshedTail(current.value, next)
            : next,
        }));
        if (!isBackgroundRefresh) setHistoryGeneration(0);
      })
      .catch((err) => {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : 'Unable to open this product conversation.');
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => { cancelled = true; };
  }, [productConversationId, snapshotRetry]);

  useEffect(() => {
    const notificationIds = new Set([
      productConversationId,
      snapshot?.latest_transcript_row_id,
      snapshot?.canonical_root.transcript_row_id,
      ...((snapshot?.segments ?? []).map((segment) => segment.transcript_row_id)),
    ].filter((id): id is string => Boolean(id)));
    if (notificationIds.size === 0) return;
    const refresh = (source: 'close' | 'stream') => {
      const closeIsActive = snapshot?.close != null && snapshot.close.phase !== 'completed';
      if (source === 'close' || closeIsActive) setSnapshotRetry((retry) => retry + 1);
    };
    const unsubscribes = [...notificationIds].map((id) =>
      subscribeCloseSnapshotChanged(id, refresh));
    return () => unsubscribes.forEach((unsubscribe) => unsubscribe());
  }, [productConversationId, snapshot]);

  useEffect(() => {
    if (!latestProjection?.conversationId || !snapshot) return;
    const previous = observedMemberProjectionRef.current;
    observedMemberProjectionRef.current = latestProjection;
    const lifecycleMismatch = latestProjection.serverArchived
      !== (snapshot.ordinary_lifecycle === 'history');
    const projectionChanged = previous !== latestProjection;
    if (lifecycleMismatch && projectionChanged) {
      setSnapshotRetry((retry) => retry + 1);
    }
  }, [latestProjection, snapshot]);

  useEffect(() => {
    const projection = latestProjection;
    if (!projection?.conversationId || snapshot?.ordinary_lifecycle === 'history') {
      setTaskApprovalOverlay(null);
      setApprovalContextWindowUsed(null);
      return;
    }
    const phase = projection.convState;
    if (phase.type !== 'awaiting_task_approval') {
      setTaskApprovalOverlay(null);
      setApprovalContextWindowUsed(null);
      return;
    }
    setTaskApprovalOverlay({
      title: phase.title,
      priority: phase.priority as TaskApprovalOverlayState['priority'],
      plan: phase.plan,
      conversationId: projection.conversationId,
    });
    let cancelled = false;
    setApprovalContextWindowUsed(null);
    api.getConversation(projection.conversationId)
      .then((result) => {
        if (!cancelled) setApprovalContextWindowUsed(result.context_window_size);
      })
      .catch(() => {
        if (!cancelled) setApprovalContextWindowUsed(null);
      });
    return () => {
      cancelled = true;
    };
  }, [latestProjection, snapshot?.ordinary_lifecycle]);

  useEffect(() => {
    const handler = (event: Event) => {
      const { sequenceId, messageId, occurrenceToken, presentation } = (event as CustomEvent<OpenMessageViewerEventDetail>).detail ?? {};
      if (!Number.isSafeInteger(sequenceId) || sequenceId <= 0) return;
      if (presentation !== 'pane' && presentation !== 'fullscreen') return;
      viewerSlot.openMessage(sequenceId, presentation, messageId, occurrenceToken);
    };
    window.addEventListener(OPEN_MESSAGE_VIEWER_EVENT, handler);
    return () => window.removeEventListener(OPEN_MESSAGE_VIEWER_EVENT, handler);
  }, [viewerSlot]);

  const loadOlderMessages = useCallback(async (restoreBasis?: RestoreBasis) => {
    if (!productConversationId || !snapshot?.has_older || !snapshot.before || loadingOlder) return;
    const ownerId = productConversationId;
    const routeGeneration = routeGenerationRef.current;
    const requestGeneration = ++paginationRequestRef.current;
    const requestView = {
      conversationId: latestProjection?.conversationId ?? snapshot.product_conversation_id,
      generation: historyGeneration,
      transcriptGeneration: historyGeneration,
    };
    setRestoreCommand(null);
    setLoadingOlder(true);
    setOlderError(null);
    try {
      const { older, refreshedTail } = await fetchOlderSnapshotWithFreshCursor(productConversationId, snapshot.before);
      if (routeGenerationRef.current !== routeGeneration || paginationRequestRef.current !== requestGeneration) return;
      setOwnedSnapshot((current) => {
        if (!current || current.productConversationId !== ownerId) return current;
        const authoritativeTail = refreshedTail
          ? mergeRefreshedTail(current.value, refreshedTail)
          : current.value;
        return { productConversationId: ownerId, value: mergeOlderSegments(authoritativeTail, older) };
      });
      setHistoryGeneration((generation) => generation + 1);
      if (restoreBasis?.kind === 'reader_anchor') {
        setRestoreCommand({
          kind: 'positioning',
          command: {
            kind: 'restore_after_prefix_expansion',
            token: requestGeneration,
            requestToken: requestGeneration,
            view: {
              ...requestView,
              generation: requestView.generation + 1,
              transcriptGeneration: requestView.transcriptGeneration + 1,
            },
            messageId: restoreBasis.messageId,
            viewportStartOffset: restoreBasis.viewportStartOffset,
          },
        });
      }
    } catch (err) {
      if (routeGenerationRef.current === routeGeneration && paginationRequestRef.current === requestGeneration) {
        setOlderError(err instanceof Error ? err.message : 'Failed to load earlier history.');
      }
    } finally {
      if (routeGenerationRef.current === routeGeneration && paginationRequestRef.current === requestGeneration) {
        setLoadingOlder(false);
      }
    }
  }, [historyGeneration, latestProjection?.conversationId, loadingOlder, productConversationId, snapshot]);

  const messages = aggregateMessages;
  const closeInProgress = snapshot?.close != null && snapshot.close.phase !== 'completed';
  const convState = useMemo(
    () => snapshot?.ordinary_lifecycle === 'history' || closeInProgress
      ? ({ type: 'idle' } satisfies ConversationState)
      : latestProjection?.convState ?? (snapshot ? aggregateConversationState(snapshot, loadingOlder, olderError) : { type: 'idle' } satisfies ConversationState),
    [closeInProgress, latestProjection, loadingOlder, olderError, snapshot],
  );
  const latestSlug = latestProjection?.slug ?? snapshot?.latest_transcript_row_id ?? null;
  const latestConversationId = latestProjection?.conversationId ?? snapshot?.latest_transcript_row_id ?? undefined;
  const latestWorkScopeKey = snapshot?.ordinary_lifecycle === 'open'
    ? latestProjection?.conversation?.work_scope_key
    : undefined;
  const transcriptView = {
    conversationId: latestConversationId ?? snapshot?.product_conversation_id ?? '',
    generation: historyGeneration,
    transcriptGeneration: historyGeneration,
  };
  const hashTargetLoaded = !!hashTargetMessageId && messages.some((message) => message.message_id === hashTargetMessageId);
  const hashTargetExhausted = !!hashTargetMessageId && !hashTargetLoaded && snapshot?.has_older === false;
  const transcriptPositioning = restoreCommand ?? (hashTargetLoaded
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
    : { kind: 'idle' as const, view: transcriptView });
  const isOpen = snapshot?.ordinary_lifecycle === 'open';
  const liveControlsEnabled = isOpen && !closeInProgress;

  useEffect(() => {
    if (!hashTargetMessageId || hashTargetLoaded || !snapshot?.has_older || loadingOlder || olderError) return;
    void loadOlderMessages();
  }, [hashTargetLoaded, hashTargetMessageId, loadOlderMessages, loadingOlder, olderError, snapshot?.has_older]);

  if (loading) {
    return (
      <main className="product-conversation-page" data-testid="product-conversation-page">
        <section id="chat-view" className="view active product-conversation-page__transcript">
          <div id="messages"><MessageListSkeleton count={6} /></div>
        </section>
      </main>
    );
  }

  if (!snapshot) {
    const fallbackSlug = productConversationId;
    return (
      <main className="product-conversation-page">
        <div role="alert">{error ?? 'Unable to open this product conversation.'} Showing cached row if available.</div>
        <button className="btn-secondary" type="button" onClick={() => setSnapshotRetry((retry) => retry + 1)}>Retry</button>
        {fallbackSlug && (
          <EmbeddedConversationPage
            slug={fallbackSlug}
            routePrefix="/c"
            suppressCanonicalization
            mutationEnabled={false}
            aggregateLifecycleOpen={false}
          />
        )}
      </main>
    );
  }

  return (
    <main
      className={`product-conversation-page${showSplitPaneViewer ? ' product-conversation-page--split-pane' : ''}`}
      data-testid="product-conversation-page"
    >
      <ProductConversationHeader
        snapshot={snapshot}
        productConversationId={snapshot.product_conversation_id}
        messages={messages}
        recallDisabled={!liveControlsEnabled}
      />
      {(olderError || error || hashTargetExhausted) && (
        <div className="product-conversation-page__status" role="alert">
          {olderError ?? error ?? 'The linked message is not available in this conversation history.'}
        </div>
      )}
      <section className="view active product-conversation-page__transcript" data-testid="product-conversation-transcript">
        <ConversationNavStack
          messages={messages}
          pendingMessages={latestProjection?.pendingMessages ?? []}
          convState={convState}
          onRetry={latestProjection?.onRetryPending ?? (() => {})}
          onCancelSteering={latestProjection?.onCancelSteering}
          onOpenFile={latestProjection?.onOpenFile}
          filePathRootDir={latestProjection?.filePathRootDir}
          systemPrompt={latestProjection?.systemPrompt}
          enableMessageSidepanel
          enableMessageFullscreen
          conversationId={latestConversationId ?? snapshot.product_conversation_id}
          slug={latestSlug ?? snapshot.canonical_root.slug ?? snapshot.requested_transcript_row_id}
          hasOlderMessages={snapshot.has_older}
          onLoadOlderMessages={snapshot.has_older ? (restoreBasis) => { void loadOlderMessages(restoreBasis); } : undefined}
          loadingOlderMessages={loadingOlder}
          olderHistoryError={olderError}
          transcriptPositioning={transcriptPositioning}
          onHistoryScrollCommandHandled={(token) => {
            setRestoreCommand((current) => (
              current?.kind === 'positioning' && current.command.token === token ? null : current
            ));
          }}
          {...(latestWorkScopeKey ? { workScopeKey: latestWorkScopeKey } : {})}
        />
      </section>
      {isOpen && snapshot.latest_transcript_row_id ? (
        <div className="product-conversation-page__composer" data-testid="product-conversation-composer">
          <EmbeddedConversationPage
            slug={snapshot.latest_transcript_row_id}
            showTranscript={false}
            embeddedShell
            suppressCanonicalization={true}
            suppressMessageViewerOwner
            suppressTaskApprovalOwner={true}
            mutationEnabled={liveControlsEnabled}
            aggregateLifecycleOpen={isOpen}
            onProjectionChange={setLatestProjection}
            onCloseCompleted={() => setSnapshotRetry((retry) => retry + 1)}
          />
        </div>
      ) : (
        <div className="product-conversation-page__composer-placeholder" data-testid="product-conversation-history">History is read-only.</div>
      )}

      {aggregateMessageSlot && (
        <ReviewNotesProvider scopeKey={latestConversationId ?? snapshot.product_conversation_id}>
          <Suspense fallback={null}>
            <div className={showSplitPaneViewer ? 'product-conversation-page__viewer-pane' : undefined}>
              <MessageViewer
                sequenceId={aggregateMessageSlot.sequenceId}
                messageId={aggregateMessageSlot.messageId}
                occurrenceToken={aggregateMessageSlot.occurrenceToken}
                messages={aggregateMessages}
                onClose={closeAggregateMessageViewer}
                onSendNotes={liveControlsEnabled && latestProjection?.appendReviewNotesToComposer
                  ? appendAggregateReviewNotes
                  : undefined}
                presentation={aggregateMessageSlot.presentation}
                canTogglePresentation={isWideDesktop}
                onPresentationChange={viewerSlot.setPresentation}
                inline={showSplitPaneViewer}
              />
            </div>
          </Suspense>
        </ReviewNotesProvider>
      )}

        {taskApprovalOverlay && latestProjection?.conversationId === taskApprovalOverlay.conversationId && (
          <Suspense fallback={null}>
            <TaskApprovalReader
              title={taskApprovalOverlay.title}
              priority={taskApprovalOverlay.priority}
              plan={taskApprovalOverlay.plan}
              contextWindowUsed={approvalContextWindowUsed ?? undefined}
              modelContextWindow={latestProjection.modelContextWindow}
              mutationEnabled={liveControlsEnabled}
              {...(liveControlsEnabled ? {
                onApprove: (handoff) => api.approveTask(taskApprovalOverlay.conversationId, handoff)
                  .then((result) => { if (result.first_task) setShowFirstTaskWelcome(true); })
                  .catch(() => {}),
                onReject: () => api.rejectTask(taskApprovalOverlay.conversationId).then(() => {}).catch(() => {}),
                onSendFeedback: (annotations: string) => api.sendTaskFeedback(taskApprovalOverlay.conversationId, annotations).then(() => {}).catch(() => {}),
              } : {
                onApprove: () => {},
                onReject: () => {},
                onSendFeedback: () => {},
              })}
            />
          </Suspense>
        )}

        {showFirstTaskWelcome && (
          <Suspense fallback={null}>
            <FirstTaskWelcome visible onClose={() => setShowFirstTaskWelcome(false)} />
          </Suspense>
        )}
    </main>
  );
}
