import { Suspense, lazy, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { useLocation, useParams } from 'react-router-dom';
import { decodedRouteReference } from '../transcriptReference';
import { ConversationNavStack } from '../components/ConversationNavStack';
import { ChainWorkIdentityBlock } from '../components/ChainWorkIdentityBlock';
import { AutomaticContinuationControl } from '../components/AutomaticContinuationControl';
import { ConversationGoalDetails, ConversationGoalSummary } from '../components/ConversationGoals';
import { useConversationGoals } from '../hooks/useConversationGoals';
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
  type ProductConversationHistorySnapshotView as ProductConversationSnapshotView,
} from '../api';
import { useChainAtom, type InflightQa } from '../chain';
import { parseConversationState } from '../utils';
import type { EnrichedMessage } from '../generated/EnrichedMessage';
import type { HistoryScrollCommand, RestoreBasis } from '../conversation/historyExpansion';
import { transcriptPositioningInputCommandKey, type TranscriptPositioningCompletion, type TranscriptPositioningInput } from '../conversation/transcriptPositioning';
import { OPEN_MESSAGE_VIEWER_EVENT, type OpenMessageViewerEventDetail } from '../components/MessageContextMenu';
import { useReaderNavigationKey, useViewerSlot } from '../contexts/ViewerSlotContext';
import { ReviewNotesProvider } from '../contexts/ReviewNotesContext';
import { useIsWideDesktop } from '../hooks/useMediaQuery';
import { useConnection } from '../hooks/useConnection';
import type { SSEAction } from '../conversation/atom';
import { ConversationContext } from '../conversation/ConversationContext';
import { useConversationTranscriptAuthority, useConversationTranscriptView } from '../conversation/useConversationAtom';
import { EmbeddedConversationPage, type EmbeddedConversationProjection } from './ConversationPage';
import {
  getProductConversationSnapshotChangeSequence,
  getProductConversationDeleteSequence,
  productConversationSnapshotChangedSince,
  productConversationDeletedSince,
  subscribeCloseSnapshotChanged,
  subscribeProductConversationDeleted,
  subscribeProductConversationSnapshotChanged,
  subscribeProductConversationsReconciled,
} from '../notifications';
import { generateUUID } from '../utils/uuid';
import { useConversationViewJourney } from '../hooks/useConversationViewJourney';
import type { ViewJourneyLifecycle } from '../hooks/conversationViewLifecycle';
import type { ConversationHistorySegmentView } from '../generated/ConversationHistorySegmentView';
import type { ConversationHistoryBoundaryView } from '../generated/ConversationHistoryBoundaryView';
import type { CoordinatorConversationSnapshotView } from '../generated/CoordinatorConversationSnapshotView';
import type { ComponentProps } from 'react';
import './ProductConversationPage.css';
import type { ContinuationOpeningView } from '../generated/ContinuationOpeningView';
import { reconcileOpening, sameOpeningEdge } from '../conversation/opening';

type AcquiredMessage = EnrichedMessage | Message;
type ProductConversationSegmentView = Omit<ConversationHistorySegmentView, 'messages'> & { messages: AcquiredMessage[] };
type HistorySnapshot = (
  | (Omit<ProductConversationSnapshotView, 'segments'> & { kind: 'ordinary' })
  | (Omit<CoordinatorConversationSnapshotView, 'segments'> & { kind: 'coordinator' })
) & { segments: ProductConversationSegmentView[] };
type OrdinaryHistorySnapshot = Extract<HistorySnapshot, { kind: 'ordinary' }>;
type HistoryOptions = Parameters<typeof api.getProductConversationHistorySnapshot>[1];
async function fetchOrdinarySnapshot(reference: string, options: HistoryOptions): Promise<HistorySnapshot> {
  return { kind: 'ordinary', ...await api.getProductConversationHistorySnapshot(reference, options) };
}
async function fetchCoordinatorSnapshot(reference: string, options: HistoryOptions): Promise<HistorySnapshot> {
  return { kind: 'coordinator', ...await api.getCoordinatorHistory(reference, options) };
}
type HistoryPageProps = { productId?: string | undefined; journey?: ViewJourneyLifecycle | undefined; globalShell?: { composerQuickAction: ComponentProps<typeof EmbeddedConversationPage>['composerQuickAction']; stateBarExtension: ComponentProps<typeof EmbeddedConversationPage>['stateBarExtension'] } };


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
  import('../components/ChainQaColumn').then((m) => ({ default: m.ChainQaColumn })),
);


const discardCloseLiveUpdateAction = (_action: SSEAction): void => {
  void _action;
};

function CloseLiveUpdateOwner({ conversationId }: { conversationId: string }) {
  useConnection({ conversationId, dispatch: discardCloseLiveUpdateAction });
  return null;
}

function RetryTranscriptHydrationOwner({ conversationId }: { conversationId: string }) {
  const store = useContext(ConversationContext);
  if (!store) throw new Error('Retry transcript hydration requires ConversationProvider');
  const dispatch = useCallback((action: SSEAction) => {
    store.dispatch(conversationId, action);
  }, [conversationId, store]);
  // No replay cursor: this owner requests message-bearing hydration, not preserve-only init.
  useConnection({ conversationId, dispatch });
  return null;
}

async function fetchOlderSnapshotWithFreshCursor(
  productConversationId: string,
  before: string,
  coordinator: boolean,
  onFreshTailRequest: () => void,
  requestOptions: (purpose: 'older' | 'recovery') => HistoryOptions,
): Promise<{ older: HistorySnapshot; refreshedTail?: HistorySnapshot }> {
  try {
    return { older: await (coordinator ? fetchCoordinatorSnapshot : fetchOrdinarySnapshot)(productConversationId, {
      message_limit: PAGE_SIZE,
      before,
      ...requestOptions('older'),
    }) };
  } catch (error) {
    if (!(error instanceof ApiResponseError) || error.status !== 400) throw error;
    onFreshTailRequest();
    const refreshed = await (coordinator ? fetchCoordinatorSnapshot : fetchOrdinarySnapshot)(productConversationId, { message_limit: PAGE_SIZE, ...requestOptions('recovery') });
    if (!refreshed.has_older || !refreshed.before) throw error;
    return {
      refreshedTail: refreshed,
      older: await (coordinator ? fetchCoordinatorSnapshot : fetchOrdinarySnapshot)(productConversationId, {
        message_limit: PAGE_SIZE,
        before: refreshed.before,
        ...requestOptions('older'),
      }),
    };
  }
}

type TranscriptReaderSelection =
  | { kind: 'latest' }
  | { kind: 'transcript'; segment: ProductConversationSegmentView }
  | { kind: 'unavailable' };

function transcriptReaderSelection(snapshot: HistorySnapshot, search: string, routeTranscriptPin: string | null): TranscriptReaderSelection {
  const pins = new URLSearchParams(search).getAll('source_transcript');
  if (pins.length > 0) {
    const pin = pins[0];
    if (pins.length !== 1 || !pin || pin.trim() !== pin) return { kind: 'unavailable' };
    const segment = snapshot.segments.find((candidate) => candidate.transcript_row_id === pin);
    return segment ? { kind: 'transcript', segment } : { kind: 'unavailable' };
  }
  if (routeTranscriptPin !== null) {
    const segment = snapshot.segments.find((candidate) => candidate.transcript_row_id === routeTranscriptPin);
    return segment ? { kind: 'transcript', segment } : { kind: 'unavailable' };
  }
  return { kind: 'latest' };
}

type OpeningObservation = {
  transcriptRowId: string;
  opening: ContinuationOpeningView | null;
};

type OpeningProvenance = {
  source: 'snapshot' | 'stream';
  observation: OpeningObservation | null;
};

function sameOpeningObservation(left: OpeningObservation | null, right: OpeningObservation | null): boolean {
  return left === right || (!!left && !!right && left.transcriptRowId === right.transcriptRowId
    && left.opening === right.opening);
}

type OwnedSnapshot = {
  productConversationId: string;
  value: HistorySnapshot;
  openingProvenance: OpeningProvenance;
};

type ProductConversationOpenMeasurement = {
  openId: string;
  routeReference: string;
  request: Promise<HistorySnapshot> | undefined;
  startedAt: number;
  snapshotReceivedAt?: number;
  storeReadyAt?: number;
  initiallyVisible: boolean;
  reported: boolean;
};

function reportProductConversationOpen(measurement: ProductConversationOpenMeasurement, paintedAt: number | null): void {
  if (measurement.reported || measurement.snapshotReceivedAt === undefined || measurement.storeReadyAt === undefined) return;
  measurement.reported = true;
  void api.reportProductConversationOpen({
    open_id: measurement.openId,
    snapshot_received_ms: measurement.snapshotReceivedAt - measurement.startedAt,
    store_ready_ms: measurement.storeReadyAt - measurement.startedAt,
    first_paint_ms: paintedAt === null ? null : paintedAt - measurement.startedAt,
    total_ms: performance.now() - measurement.startedAt,
    visible: measurement.initiallyVisible,
  }).catch(() => {});
}

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

function toMessage(message: AcquiredMessage, occurrenceToken?: string): Message {
  return {
    message_id: message.message_id,
    conversation_id: message.conversation_id,
    sequence_id: message.sequence_id,
    message_type: message.message_type,
    content: message.content as Message['content'],
    ...(message.origin === undefined ? {} : { origin: message.origin }),
    event_presentation: message.event_presentation,
    display_data: occurrenceToken
      ? {
        ...(((message.display_data ?? null) as Exclude<Message['display_data'], undefined>) ?? {}),
        productOccurrenceToken: occurrenceToken,
      }
      : (message.display_data ?? null) as Exclude<Message['display_data'], undefined>,
    usage_data: message.usage_data ?? null,
    created_at: message.created_at,
  };
}

function sortSegments(snapshot: HistorySnapshot) {
  return snapshot.segments
    .slice()
    .sort((a, b) => a.segment_ordinal - b.segment_ordinal);
}

function toSegmentMessage(segment: ProductConversationSegmentView, enriched: AcquiredMessage, boundaries: ConversationHistoryBoundaryView[]): Message {
  const message = toMessage(enriched, productOccurrenceToken(segment.transcript_row_id, enriched.message_id));
  const boundary = boundaries.find((candidate) => candidate.message_id === message.message_id);
  if (!boundary) return message;
  return { ...message, display_data: { ...message.display_data, productHistoricalHandoff: {
    predecessor_transcript_row_id: boundary.predecessor_transcript_row_id,
    successor_transcript_row_id: boundary.successor_transcript_row_id,
    continuation_message_id: message.message_id,
    segment_ordinal: boundary.segment_ordinal,
  } } };
}

function isLatestSegment(snapshot: HistorySnapshot, transcriptRowId: string): boolean {
  return transcriptRowId === snapshot.latest_transcript_row_id;
}

function representedOpeningBoundary(
  boundaries: ConversationHistoryBoundaryView[],
  target: { kind: 'message'; messageId: string } | { kind: 'transcript'; transcriptRowId: string },
): ConversationHistoryBoundaryView | undefined {
  return boundaries.find(boundary => boundary.represented_successor_opening_message_id !== null && (
    target.kind === 'message'
      ? boundary.represented_successor_opening_message_id === target.messageId
      : boundary.successor_transcript_row_id === target.transcriptRowId
  ));
}

function reconcileSnapshotOpening(snapshot: HistorySnapshot, observation: ContinuationOpeningView | null): HistorySnapshot {
  if (!observation || observation.status === 'none') return snapshot;
  const edge = observation.edge;
  const boundary = snapshot.boundaries.find(item => item.message_id === edge.continuation_message_id
    && item.predecessor_transcript_row_id === edge.predecessor_transcript_row_id
    && item.successor_transcript_row_id === edge.successor_transcript_row_id);
  if (edge.product_conversation_id !== snapshot.product_conversation_id
    || edge.successor_transcript_row_id !== snapshot.latest_transcript_row_id
    || !snapshot.segments.some(segment => segment.transcript_row_id === edge.successor_transcript_row_id)
    || !snapshot.segments.some(segment => segment.transcript_row_id === edge.predecessor_transcript_row_id)
    || !boundary) return snapshot;
  const opening = reconcileOpening(snapshot.latest_opening, observation)!;
  const represented = opening.status === 'accepted' && opening.presentation === 'boundary' ? opening.accepted_message_id : null;
  if (snapshot.latest_opening === opening && boundary.represented_successor_opening_message_id === represented) return snapshot;
  return { ...snapshot, latest_opening: opening, boundaries: snapshot.boundaries.map(item => item === boundary
    ? { ...item, represented_successor_opening_message_id: represented } : item) };
}

function deferPendingOpening(messages: Message[], opening: ContinuationOpeningView): Message[] {
  if (opening.status !== 'pending') return messages;
  return messages.filter(message => message.conversation_id !== opening.edge.successor_transcript_row_id
    || (message.message_id !== opening.client_turn_key
      && message.message_id !== `${opening.edge.successor_transcript_row_id}:${opening.client_turn_key}`));
}

function omitRepresentedOpenings<T extends AcquiredMessage>(messages: T[], boundaries: ConversationHistoryBoundaryView[]): T[] {
  return messages.filter(message => !boundaries.some(boundary => boundary.successor_transcript_row_id === message.conversation_id
    && boundary.represented_successor_opening_message_id === message.message_id));
}

function flattenHistoricalMessages(snapshot: HistorySnapshot): Message[] {
  return sortSegments(snapshot)
    .flatMap((segment) => {
      if (isLatestSegment(snapshot, segment.transcript_row_id)) return [];
      return omitRepresentedOpenings(segment.messages, snapshot.boundaries).map((message) => toSegmentMessage(segment, message, snapshot.boundaries));
    });
}

function compareTranscriptRowMessages(left: Message, right: Message): number {
  return left.sequence_id - right.sequence_id
    || left.message_id.localeCompare(right.message_id);
}

function reconcileTranscriptRowMessages(
  snapshotMessages: Message[],
  liveMessages: Message[],
  transcriptRowId: string,
): Message[] {
  const messagesById = new Map<string, Message>();
  const messageIdBySequence = new Map<number, string>();
  const insert = (message: Message, preferOccurrenceFrom?: Message) => {
    const priorAtSequence = messageIdBySequence.get(message.sequence_id);
    if (priorAtSequence && priorAtSequence !== message.message_id) {
      messagesById.delete(priorAtSequence);
    }
    const priorById = messagesById.get(message.message_id);
    if (priorById && priorById.sequence_id !== message.sequence_id) {
      messageIdBySequence.delete(priorById.sequence_id);
    }
    messagesById.set(message.message_id, {
      ...message,
      display_data: {
        ...(message.display_data ?? {}),
        productOccurrenceToken: preferOccurrenceFrom
          ? (preferOccurrenceFrom.display_data as { productOccurrenceToken?: string } | null | undefined)?.productOccurrenceToken
            ?? productOccurrenceToken(transcriptRowId, message.message_id)
          : productOccurrenceToken(transcriptRowId, message.message_id),
      },
    });
    messageIdBySequence.set(message.sequence_id, message.message_id);
  };
  for (const snapshotMessage of snapshotMessages) insert(snapshotMessage, snapshotMessage);
  for (const liveMessage of liveMessages) insert(liveMessage, messagesById.get(liveMessage.message_id));
  return [...messagesById.values()].sort(compareTranscriptRowMessages);
}

function makeAggregateMessages(
  snapshot: HistorySnapshot,
  liveMessages: Message[] | null,
): Message[] {
  const historical = flattenHistoricalMessages(snapshot);
  if (!liveMessages) {
    const latestSegment = sortSegments(snapshot).find((segment) => isLatestSegment(snapshot, segment.transcript_row_id));
    const latestMessages = latestSegment
      ? omitRepresentedOpenings(latestSegment.messages, snapshot.boundaries).map((message) => toSegmentMessage(latestSegment, message, snapshot.boundaries))
      : [];
    return [...historical, ...latestMessages];
  }
  const latestSegment = sortSegments(snapshot).find((segment) => isLatestSegment(snapshot, segment.transcript_row_id));
  const latestSnapshotMessages = latestSegment ? omitRepresentedOpenings(latestSegment.messages, snapshot.boundaries).map((message) => toSegmentMessage(latestSegment, message, snapshot.boundaries)) : [];
  return [
    ...historical,
    ...reconcileTranscriptRowMessages(
      latestSnapshotMessages,
      omitRepresentedOpenings(liveMessages, snapshot.boundaries),
      snapshot.latest_transcript_row_id,
    ),
  ];
}

function mergeOlderSegments(
  current: HistorySnapshot,
  older: Pick<HistorySnapshot, 'segments' | 'before' | 'has_older'>,
): HistorySnapshot {
  const mergedByRowId = new Map(current.segments.map((segment) => [segment.transcript_row_id, segment]));

  for (const olderSegment of older.segments) {
    const currentSegment = mergedByRowId.get(olderSegment.transcript_row_id);
    if (!currentSegment) {
      mergedByRowId.set(olderSegment.transcript_row_id, olderSegment);
      continue;
    }

    mergedByRowId.set(olderSegment.transcript_row_id, {
      ...currentSegment,
      ...olderSegment,
      messages: reconcileTranscriptRowMessages(
        olderSegment.messages.map(message => toSegmentMessage(olderSegment, message, current.boundaries)),
        currentSegment.messages.map(message => toSegmentMessage(currentSegment, message, current.boundaries)),
        currentSegment.transcript_row_id,
      ),
    });
  }

  return {
    ...current,
    segments: Array.from(mergedByRowId.values()).sort((a, b) => a.segment_ordinal - b.segment_ordinal),
    before: older.before,
    has_older: older.has_older,
  };
}

function mergeRefreshedTail(
  current: HistorySnapshot,
  refreshed: HistorySnapshot,
): HistorySnapshot {
  if (current.latest_opening.status === 'accepted' && refreshed.latest_opening.status !== 'accepted'
    && current.latest_transcript_row_id === refreshed.latest_transcript_row_id) {
    refreshed = reconcileSnapshotOpening(refreshed, current.latest_opening);
  }
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
    segmentsByRowId.set(retainedSegment.transcript_row_id, {
      ...retainedSegment,
      ...refreshedSegment,
      messages: reconcileTranscriptRowMessages(
        retainedSegment.messages.map(message => toSegmentMessage(retainedSegment, message, current.boundaries)),
        refreshedSegment.messages.map(message => toSegmentMessage(refreshedSegment, message, refreshed.boundaries)),
        retainedSegment.transcript_row_id,
      ),
    });
  }
  const firstRefreshedSegment = sortSegments(refreshed).find(segment => segment.messages.length > 0);
  const firstRefreshedMessage = firstRefreshedSegment?.messages.slice()
    .sort((a, b) => a.sequence_id - b.sequence_id || a.message_id.localeCompare(b.message_id))[0];
  const sameTopology = current.segments.length === refreshed.segments.length
    && refreshed.segments.every(segment => current.segments.some(retained => (
      retained.transcript_row_id === segment.transcript_row_id && retained.segment_ordinal === segment.segment_ordinal
    )));
  const retainedPrefix = sameTopology && firstRefreshedSegment !== undefined && firstRefreshedMessage !== undefined
    && current.segments.some(segment => segment.transcript_row_id === firstRefreshedSegment.transcript_row_id
      && segment.messages.some(message => message.message_id === firstRefreshedMessage.message_id
        && message.sequence_id === firstRefreshedMessage.sequence_id))
    && current.segments.some(segment => segment.messages.some(message => (
      segment.segment_ordinal < firstRefreshedSegment.segment_ordinal
      || (segment.segment_ordinal === firstRefreshedSegment.segment_ordinal
        && (message.sequence_id < firstRefreshedMessage.sequence_id
          || (message.sequence_id === firstRefreshedMessage.sequence_id && message.message_id < firstRefreshedMessage.message_id)))
    )));
  return {
    ...refreshed,
    segments: Array.from(segmentsByRowId.values())
      .sort((a, b) => a.segment_ordinal - b.segment_ordinal),
    before: retainedPrefix ? current.before : refreshed.before,
    has_older: retainedPrefix ? current.has_older : refreshed.has_older,
  };
}

function aggregateConversationState(snapshot: HistorySnapshot, loadingOlder: boolean, olderError: string | null): ConversationState {
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

function countSnapshotMessages(snapshot: Pick<HistorySnapshot, 'segments'>): number {
  return snapshot.segments.reduce((sum, segment) => sum + segment.messages.length, 0);
}

function chainFromSnapshot(snapshot: OrdinaryHistorySnapshot, qaHistory: ChainQaRow[]): ChainView {
  return {
    root_conv_id: snapshot.chain_qa_compatibility?.root_transcript_row_id ?? snapshot.requested_transcript_row_id,
    product_conversation_id: snapshot.product_conversation_id,
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
  snapshot: OrdinaryHistorySnapshot;
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
      } else if (evt.type === 'chain_qa_answer_reset') {
        dispatch({ type: 'ANSWER_RESET', chainQaId: evt.chain_qa_id });
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
  snapshot: OrdinaryHistorySnapshot;
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
  snapshot: OrdinaryHistorySnapshot;
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
            <a href={`/c/${encodeURIComponent(source.source_product_conversation_id)}`}>
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
        <AutomaticContinuationControl
          scope={{ kind: 'ordinary', reference: snapshot.product_conversation_id }}
        />
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

export function ProductConversationPage(props: HistoryPageProps = {}) {
  return <ProductConversationPageInner {...props} />;
}

function ProductConversationPageInner({ productId, globalShell, journey: inheritedJourney }: HistoryPageProps) {
  const coordinator = globalShell !== undefined;
  const journey = useConversationViewJourney(coordinator ? 'global' : 'ordinary', inheritedJourney);
  const journeyRef = useRef(journey);
  useLayoutEffect(() => { journeyRef.current = journey; }, [journey]);
  const fetchSnapshot = coordinator ? fetchCoordinatorSnapshot : fetchOrdinarySnapshot;
  const conversationStore = useContext(ConversationContext);
  if (!conversationStore) throw new Error('Product conversation requires ConversationProvider');
  const params = useParams<{ productConversationId: string; slug: string }>();
  const productConversationId = productId ?? params.productConversationId;
  const viewerSlot = useViewerSlot();
  const location = useLocation();
  const routeReference = params.slug === undefined ? undefined : decodedRouteReference(location.pathname);
  const hashTargetMessageId = decodeMessageHash(location.hash);
  const scrollCommandTokenRef = useRef(0);
  const readerNavigationKey = useReaderNavigationKey();
  const hashNavigationRef = useRef({ key: '', token: 0 });
  const committedHashNavigation = hashNavigationRef.current;
  const nextScrollCommandToken = scrollCommandTokenRef.current + 1;
  const hashNavigation = useMemo(() => committedHashNavigation.key === readerNavigationKey ? committedHashNavigation : {
    key: readerNavigationKey,
    token: nextScrollCommandToken,
  }, [committedHashNavigation, readerNavigationKey, nextScrollCommandToken]);
  useLayoutEffect(() => {
    hashNavigationRef.current = hashNavigation;
    scrollCommandTokenRef.current = Math.max(scrollCommandTokenRef.current, hashNavigation.token);
  }, [hashNavigation]);
  const [jumpCompletion, setJumpCompletion] = useState<TranscriptPositioningCompletion | null>(null);
  const desiredJumpRef = useRef<Extract<HistoryScrollCommand, { kind: 'jump_to_message' }> | null>(null);
  const [ownedSnapshot, setOwnedSnapshot] = useState<OwnedSnapshot | null>(null);
  const ownedSnapshotRef = useRef<OwnedSnapshot | null>(null);
  const snapshotChangeSequenceRef = useRef(0);
  const baseSnapshot = ownedSnapshot && ownedSnapshot.productConversationId === productConversationId
    ? ownedSnapshot.value
    : null;
  const baseOrdinarySnapshot = baseSnapshot?.kind === 'ordinary' ? baseSnapshot : null;
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [snapshotRetry, setSnapshotRetry] = useState(0);
  useEffect(() => {
    const refresh = () => setSnapshotRetry((value) => value + 1);
    window.addEventListener('phoenix:automatic-continuation-updated', refresh);
    return () => window.removeEventListener('phoenix:automatic-continuation-updated', refresh);
  }, []);
  useEffect(() => {
    setRestoreCommand(null);
    setOlderError(null);
  }, [readerNavigationKey]);
  const [openSnapshotGeneration, setOpenSnapshotGeneration] = useState(0);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [olderError, setOlderError] = useState<string | null>(null);
  const [latestProjection, setLatestProjection] = useState<EmbeddedConversationProjection | null>(null);
  const [taskApprovalOverlay, setTaskApprovalOverlay] = useState<TaskApprovalOverlayState | null>(null);
  const [approvalContextWindowUsed, setApprovalContextWindowUsed] = useState<number | null>(null);
  const [showFirstTaskWelcome, setShowFirstTaskWelcome] = useState(false);
  const [historyGeneration, setHistoryGeneration] = useState(0);
  const [restoreCommand, setRestoreCommand] = useState<{ navigationToken: number; positioning: TranscriptPositioningInput } | null>(null);
  const pendingOlderRestoreRef = useRef<{ requestGeneration: number; basis: RestoreBasis | undefined } | null>(null);
  const routeGenerationRef = useRef(0);
  const aggregateDeletedRef = useRef(false);
  const pendingAuthoritativeIdentitiesRef = useRef<ReadonlySet<string> | null>(null);
  const openMeasurementRef = useRef<ProductConversationOpenMeasurement | null>(null);
  if (coordinator || openMeasurementRef.current?.routeReference !== productConversationId) {
    openMeasurementRef.current = !coordinator && productConversationId ? {
      openId: generateUUID(),
      routeReference: productConversationId,
      request: undefined,
      startedAt: performance.now(),
      initiallyVisible: document.visibilityState === 'visible',
      reported: false,
    } : null;
  }
  const paginationRequestRef = useRef(0);
  const observedMemberProjectionRef = useRef<typeof latestProjection>(null);
  const transcriptAuthority = useConversationTranscriptAuthority(baseSnapshot?.latest_transcript_row_id ?? null);
  const currentLatestProjection = baseSnapshot
    && transcriptAuthority?.kind === 'live'
    && latestProjection?.conversationId === baseSnapshot.latest_transcript_row_id
    ? latestProjection
    : null;
  const isOpen = !!baseSnapshot && (!baseOrdinarySnapshot || baseOrdinarySnapshot.ordinary_lifecycle === 'open');
  const closeIncomplete = baseOrdinarySnapshot?.close?.outcome === 'close_incomplete';
  const liveTranscriptRowId = isOpen && !closeIncomplete && transcriptAuthority?.kind === 'live'
    ? baseSnapshot.latest_transcript_row_id : null;
  const memberTranscript = useConversationTranscriptView(liveTranscriptRowId);
  const currentMemberTranscript = liveTranscriptRowId && memberTranscript?.conversationId === liveTranscriptRowId
    ? memberTranscript
    : null;
  const observation: OpeningObservation | null = currentMemberTranscript?.conversationId ? {
    transcriptRowId: currentMemberTranscript.conversationId,
    opening: currentMemberTranscript.opening,
  } : null;
  let renderOwner = baseSnapshot ? ownedSnapshot : null;
  if (renderOwner && observation && !sameOpeningObservation(renderOwner.openingProvenance.observation, observation)) {
    const reconciled = reconcileSnapshotOpening(renderOwner.value, observation.opening);
    const accepted = reconciled.latest_opening;
    const openingMessage = accepted.status === 'accepted' && accepted === observation.opening
      ? currentMemberTranscript?.messages.find(message => message.message_id === accepted.accepted_message_id) : undefined;
    const value = openingMessage ? { ...reconciled, segments: reconciled.segments.map(segment =>
      segment.transcript_row_id === observation.transcriptRowId ? { ...segment,
        messages: reconcileTranscriptRowMessages(segment.messages.map(message => toSegmentMessage(segment, message, reconciled.boundaries)), [openingMessage], segment.transcript_row_id),
      } : segment) } : reconciled;
    renderOwner = { ...renderOwner, value, openingProvenance: {
      source: reconciled !== renderOwner.value || openingMessage ? 'stream' : renderOwner.openingProvenance.source,
      observation,
    } };
    setOwnedSnapshot(renderOwner);
  }
  const snapshot = renderOwner?.value ?? null;
  const ordinarySnapshot = snapshot?.kind === 'ordinary' ? snapshot : null;
  const liveMessages = useMemo(() => {
    const messages = currentMemberTranscript?.messages ?? null;
    const opening = snapshot?.latest_opening;
    const streamOpening = currentMemberTranscript?.opening;
    if (renderOwner?.openingProvenance.source !== 'snapshot' || opening?.status !== 'accepted'
      || streamOpening?.status !== 'accepted' || !sameOpeningEdge(opening.edge, streamOpening.edge)
      || opening.accepted_message_id !== streamOpening.accepted_message_id) return messages;
    return messages?.filter(message => message.message_id !== opening.accepted_message_id) ?? null;
  }, [currentMemberTranscript, renderOwner?.openingProvenance.source, snapshot?.latest_opening]);
  const aggregateMessages = useMemo(
    () => snapshot ? deferPendingOpening(makeAggregateMessages(snapshot, liveMessages), snapshot.latest_opening) : [],
    [liveMessages, snapshot],
  );
  const aggregatePendingMessages = useMemo(() => {
    const acknowledgedIds = new Set(aggregateMessages.map((message) => message.message_id));
    return (currentLatestProjection?.pendingMessages ?? []).filter((message) => (
      !(snapshot?.latest_opening.status === 'pending' && snapshot.latest_opening.edge.successor_transcript_row_id === currentLatestProjection?.conversationId && (message.localId === snapshot.latest_opening.client_turn_key || message.localId === `${snapshot.latest_opening.edge.successor_transcript_row_id}:${snapshot.latest_opening.client_turn_key}`))
      && !acknowledgedIds.has(message.localId)
      && !acknowledgedIds.has(`${currentLatestProjection?.conversationId}:${message.localId}`)
    ));
  }, [aggregateMessages, currentLatestProjection?.conversationId, currentLatestProjection?.pendingMessages, snapshot?.latest_opening]);
  const aggregateMessageSlot = viewerSlot.slot.kind === 'message' ? viewerSlot.slot : null;
  const isWideDesktop = useIsWideDesktop();
  const showSplitPaneViewer = isWideDesktop && aggregateMessageSlot?.presentation === 'pane';
  const closeAggregateMessageViewer = viewerSlot.close;
  const appendAggregateReviewNotes = useCallback((formattedNotes: string) => {
    currentLatestProjection?.appendReviewNotesToComposer?.(formattedNotes);
    if (!(isWideDesktop && aggregateMessageSlot?.presentation === 'fullscreen')) {
      closeAggregateMessageViewer();
    }
  }, [aggregateMessageSlot?.presentation, closeAggregateMessageViewer, currentLatestProjection, isWideDesktop]);
  ownedSnapshotRef.current = renderOwner;

  useEffect(() => {
    routeGenerationRef.current += 1;
    aggregateDeletedRef.current = false;
    pendingAuthoritativeIdentitiesRef.current = null;
    paginationRequestRef.current += 1;
    setLatestProjection(null);
    setRestoreCommand(null);
    setOlderError(null);
    setLoadingOlder(false);
    observedMemberProjectionRef.current = null;
  }, [productConversationId]);

  useEffect(() => {
    if (!productConversationId) return;
    if (aggregateDeletedRef.current) {
      setLoading(false);
      return;
    }
    let cancelled = false;
    const isBackgroundRefresh = ownedSnapshotRef.current?.productConversationId === productConversationId;
    if (!isBackgroundRefresh) setLoading(true);
    setError(null);
    setOlderError(null);
    const snapshotChangeSequence = getProductConversationSnapshotChangeSequence();
    const deleteSequence = getProductConversationDeleteSequence();
    snapshotChangeSequenceRef.current = snapshotChangeSequence;

    const requestRowId = ownedSnapshotRef.current?.productConversationId === productConversationId
      ? ownedSnapshotRef.current.value.latest_transcript_row_id : null;
    const requestRows = requestRowId ? [requestRowId] : conversationStore.listSnapshots().map(conversation => conversation.id);
    const requestOpenings = new Map(requestRows.map(rowId => [rowId, conversationStore.getSnapshot(rowId).opening]));
    const candidateMeasurement = openMeasurementRef.current;
    const measurement = candidateMeasurement && !candidateMeasurement.reported
      && candidateMeasurement.snapshotReceivedAt === undefined
      ? candidateMeasurement
      : null;
    const requestJourney = journeyRef.current;
    const requestId = measurement?.openId ?? requestJourney.openId;
    const viewRequestObservation = requestId && !isBackgroundRefresh ? requestJourney.requestStarted(requestId) : undefined;
    const request = measurement
      ? (measurement.request ??= fetchSnapshot(productConversationId, {
          message_limit: PAGE_SIZE,
          open_id: measurement.openId,
          ...(viewRequestObservation ? { onTelemetry: viewRequestObservation.response } : {}),
        }))
      : fetchSnapshot(productConversationId, { message_limit: PAGE_SIZE, ...(!isBackgroundRefresh && requestId ? { open_id: requestId } : {}), ...(viewRequestObservation ? { onTelemetry: viewRequestObservation.response } : {}) });
    request
      .then((next) => {
        if (cancelled || aggregateDeletedRef.current) return;
        if (productConversationDeletedSince([
          productConversationId,
          next.product_conversation_id,
          ...next.segments.map((segment) => segment.transcript_row_id),
        ], deleteSequence)) {
          aggregateDeletedRef.current = true;
          setOwnedSnapshot(null);
          setLatestProjection(null);
          setError('This product conversation was deleted.');
          return;
        }
        viewRequestObservation?.snapshot(next.product_conversation_id, next.requested_transcript_row_id);
        if (measurement) measurement.snapshotReceivedAt = performance.now();
        if (measurement) setOpenSnapshotGeneration((generation) => generation + 1);
        const previous = ownedSnapshotRef.current;
        const outgoingId = previous?.productConversationId === productConversationId
          && previous.value.latest_transcript_row_id !== next.latest_transcript_row_id
          ? previous.value.latest_transcript_row_id : null;
        const outgoingAtom = outgoingId && conversationStore.getTranscriptAuthority(outgoingId).kind === 'live'
          ? conversationStore.getSnapshot(outgoingId) : null;
        const outgoingMessages = outgoingAtom?.conversationId === outgoingId ? outgoingAtom?.messages ?? [] : [];
        if (next.kind === 'ordinary') conversationStore.observeProductConversationSnapshot(next);
        const requestObservation: OpeningObservation | null = requestOpenings.has(next.latest_transcript_row_id) ? {
          transcriptRowId: next.latest_transcript_row_id, opening: requestOpenings.get(next.latest_transcript_row_id) ?? null,
        } : null;
        setOwnedSnapshot((current) => {
          if (current?.productConversationId !== productConversationId) return { productConversationId, value: next, openingProvenance: { source: 'snapshot', observation: requestObservation } };
          const acquired = outgoingId === current.value.latest_transcript_row_id && outgoingMessages.length > 0
            ? { ...current.value, segments: current.value.segments.map(segment => segment.transcript_row_id === outgoingId
                ? { ...segment, messages: reconcileTranscriptRowMessages(segment.messages.map(message => toSegmentMessage(segment, message, current.value.boundaries)), omitRepresentedOpenings(outgoingMessages, next.boundaries), outgoingId) }
                : segment) }
            : current.value;
          const retainsAcceptance = acquired.latest_opening.status === 'accepted' && next.latest_opening.status !== 'accepted'
            && acquired.latest_transcript_row_id === next.latest_transcript_row_id;
          return { productConversationId, value: mergeRefreshedTail(acquired, next),
            openingProvenance: retainsAcceptance ? current.openingProvenance : { source: 'snapshot', observation: requestObservation } };
        });
        if (!isBackgroundRefresh) setHistoryGeneration(0);
        if (next.product_conversation_id !== productConversationId
          && productConversationSnapshotChangedSince(
            next.product_conversation_id,
            snapshotChangeSequence,
          )) {
          setSnapshotRetry((retry) => retry + 1);
        }
      })
      .catch((err) => {
        if (cancelled) return;
        viewRequestObservation?.failed();
        if (measurement && openMeasurementRef.current === measurement) {
          measurement.request = undefined;
        }
        if (err instanceof ApiResponseError && err.status === 404 && isBackgroundRefresh) {
          aggregateDeletedRef.current = true;
          routeGenerationRef.current += 1;
          paginationRequestRef.current += 1;
          setOwnedSnapshot(null);
          setLatestProjection(null);
          setError('This product conversation was deleted.');
        } else {
          setError(err instanceof Error ? err.message : 'Unable to open this product conversation.');
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [conversationStore, fetchSnapshot, productConversationId, snapshotRetry]);

  useEffect(() => {
    const canonicalId = snapshot?.product_conversation_id;
    const identities = new Set([
      productConversationId,
      canonicalId,
    ].filter((identity): identity is string => Boolean(identity)));
    let refreshed = false;
    const refresh = () => {
      if (refreshed) return;
      refreshed = true;
      setSnapshotRetry((retry) => retry + 1);
    };
    const unsubscribes = [...identities].map((identity) => (
      subscribeProductConversationSnapshotChanged(identity, refresh)
    ));
    if (canonicalId && canonicalId !== productConversationId
      && productConversationSnapshotChangedSince(
        canonicalId,
        snapshotChangeSequenceRef.current,
      )) {
      refresh();
    }
    return () => unsubscribes.forEach((unsubscribe) => unsubscribe());
  }, [productConversationId, snapshot?.product_conversation_id, snapshotRetry]);

  useEffect(() => {
    const identities = new Set([
      productConversationId,
      snapshot?.product_conversation_id,
      snapshot?.latest_transcript_row_id,
      snapshot?.canonical_root.transcript_row_id,
      ...((snapshot?.segments ?? []).map((segment) => segment.transcript_row_id)),
    ].filter((id): id is string => Boolean(id)));
    return subscribeProductConversationDeleted(identities, () => {
      aggregateDeletedRef.current = true;
      routeGenerationRef.current += 1;
      paginationRequestRef.current += 1;
      setOwnedSnapshot(null);
      setLatestProjection(null);
      setLoading(false);
      setError('This product conversation was deleted.');
    });
  }, [productConversationId, snapshot]);

  useEffect(() => subscribeProductConversationsReconciled((authoritativeIdentities) => {
    const canonicalId = snapshot?.product_conversation_id;
    if (!canonicalId) {
      pendingAuthoritativeIdentitiesRef.current = authoritativeIdentities;
      return;
    }
    pendingAuthoritativeIdentitiesRef.current = null;
    if (authoritativeIdentities.has(canonicalId)) return;
    setSnapshotRetry((retry) => retry + 1);
  }), [snapshot?.product_conversation_id]);

  useEffect(() => {
    const canonicalId = snapshot?.product_conversation_id;
    const authoritativeIdentities = pendingAuthoritativeIdentitiesRef.current;
    if (!canonicalId || !authoritativeIdentities) return;
    pendingAuthoritativeIdentitiesRef.current = null;
    if (authoritativeIdentities.has(canonicalId)) return;
    setSnapshotRetry((retry) => retry + 1);
  }, [snapshot?.product_conversation_id]);

  useEffect(() => {
    const notificationIds = new Set([
      productConversationId,
      snapshot?.latest_transcript_row_id,
      snapshot?.canonical_root.transcript_row_id,
      ...((snapshot?.segments ?? []).map((segment) => segment.transcript_row_id)),
    ].filter((id): id is string => Boolean(id)));
    if (notificationIds.size === 0) return;
    const refresh = (source: 'close' | 'stream') => {
      const current = ownedSnapshotRef.current;
      if (current && current.productConversationId !== productConversationId) return;
      const close = current?.value.kind === 'ordinary' ? current.value.close : null;
      const closeNeedsLiveRefresh = close != null
        && (close.phase !== 'completed'
          || close.outcome === 'close_incomplete'
          || close.outcome === 'archived_cleanup_attention');
      if (source === 'close' || closeNeedsLiveRefresh) setSnapshotRetry((retry) => retry + 1);
    };
    const unsubscribes = [...notificationIds].map((id) =>
      subscribeCloseSnapshotChanged(id, refresh));
    return () => unsubscribes.forEach((unsubscribe) => unsubscribe());
  }, [ordinarySnapshot?.close, productConversationId, snapshot]);

  const observedContinuationId = currentLatestProjection?.conversation?.continued_in_conv_id ?? null;

  useEffect(() => {
    if (currentLatestProjection?.conversationId !== snapshot?.latest_transcript_row_id) return;
    if (observedContinuationId) setSnapshotRetry((value) => value + 1);
  }, [currentLatestProjection?.conversationId, observedContinuationId, snapshot?.latest_transcript_row_id]);

  useEffect(() => {
    if (!currentLatestProjection?.conversationId || !ordinarySnapshot) return;
    const previous = observedMemberProjectionRef.current;
    observedMemberProjectionRef.current = currentLatestProjection;
    const lifecycleMismatch = currentLatestProjection.serverArchived
      !== (ordinarySnapshot?.ordinary_lifecycle === 'history');
    const projectionChanged = previous !== currentLatestProjection;
    if (lifecycleMismatch && projectionChanged) {
      setSnapshotRetry((retry) => retry + 1);
    }
  }, [currentLatestProjection, ordinarySnapshot]);

  useEffect(() => {
    const projection = currentLatestProjection;
    if (!projection?.conversationId || ordinarySnapshot?.ordinary_lifecycle === 'history') {
      setTaskApprovalOverlay(null);
      setApprovalContextWindowUsed(null);
      return;
    }
    const phase = currentMemberTranscript?.phase;
    if (phase?.type !== 'awaiting_task_approval') {
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
  }, [currentLatestProjection, currentMemberTranscript?.phase, ordinarySnapshot?.ordinary_lifecycle]);

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

  const updateOlderMessagesRestore = useCallback((basis: RestoreBasis) => {
    const pending = pendingOlderRestoreRef.current;
    if (pending?.requestGeneration === paginationRequestRef.current) pending.basis = basis;
  }, []);

  const loadOlderMessages = useCallback(async (restoreBasis?: RestoreBasis) => {
    if (!productConversationId || !snapshot?.has_older || !snapshot.before || loadingOlder) return;
    const ownerId = productConversationId;
    let freshTailOwner: HistorySnapshot | null = null;
    const requestOwner = journeyRef.current.captureRequestOwner();
    const requestOptions = (purpose: 'older' | 'recovery'): HistoryOptions => {
      const requestId = generateUUID();
      const observation = requestOwner(requestId);
      if (!observation) return {};
      return purpose === 'recovery'
        ? { request_id: requestId, onTelemetry: observation.response }
        : { open_id: requestId, onTelemetry: observation.response };
    };
    let freshTailOpenings = new Map<string, ContinuationOpeningView | null>();
    const routeGeneration = routeGenerationRef.current;
    const requestGeneration = ++paginationRequestRef.current;
    const pendingRestore = { requestGeneration, basis: restoreBasis };
    pendingOlderRestoreRef.current = pendingRestore;
    const requestNavigationToken = hashNavigationRef.current.token;
    const requestView = {
      conversationId: snapshot.product_conversation_id,
      generation: historyGeneration,
      transcriptGeneration: historyGeneration,
    };
    setRestoreCommand(null);
    setLoadingOlder(true);
    setOlderError(null);
    try {
      const { older, refreshedTail } = await fetchOlderSnapshotWithFreshCursor(productConversationId, snapshot.before, coordinator, () => {
        freshTailOwner = ownedSnapshotRef.current?.value ?? null;
        freshTailOpenings = new Map(conversationStore.listSnapshots().map(conversation => [
          conversation.id, conversationStore.getSnapshot(conversation.id).opening,
        ]));
      }, requestOptions);
      const requestObservation: OpeningObservation | null = refreshedTail && freshTailOpenings.has(refreshedTail.latest_transcript_row_id) ? {
        transcriptRowId: refreshedTail.latest_transcript_row_id,
        opening: freshTailOpenings.get(refreshedTail.latest_transcript_row_id) ?? null,
      } : null;
      if (routeGenerationRef.current !== routeGeneration || paginationRequestRef.current !== requestGeneration) return;
      if (ownedSnapshotRef.current?.value === freshTailOwner && refreshedTail?.kind === 'ordinary') {
        conversationStore.observeProductConversationSnapshot(refreshedTail);
      }
      setOwnedSnapshot((current) => {
        if (!current || current.productConversationId !== ownerId) return current;
        const appliesFreshTail = current.value === freshTailOwner && !!refreshedTail;
        const authoritativeTail = appliesFreshTail
          ? mergeRefreshedTail(current.value, refreshedTail)
          : current.value;
        const retainsRequestedBoundary = current.value.before === snapshot.before
          && current.value.has_older === snapshot.has_older;
        const acquiredPage = (!refreshedTail && retainsRequestedBoundary) || appliesFreshTail ? older : {
          ...older, before: current.value.before, has_older: current.value.has_older,
        };
        return { ...current, value: mergeOlderSegments(authoritativeTail, acquiredPage),
          openingProvenance: appliesFreshTail && refreshedTail?.latest_opening.status === 'accepted'
            ? { source: 'snapshot', observation: requestObservation } : current.openingProvenance };
      });
      setHistoryGeneration((generation) => generation + 1);
      const currentBasis = pendingRestore.basis;
      if (currentBasis?.kind === 'reader_anchor') {
        setRestoreCommand({
          navigationToken: requestNavigationToken,
          positioning: {
          kind: 'positioning',
          command: {
            kind: 'restore_after_prefix_expansion',
            token: ++scrollCommandTokenRef.current,
            requestToken: requestGeneration,
            view: {
              ...requestView,
              generation: requestView.generation + 1,
              transcriptGeneration: requestView.transcriptGeneration + 1,
            },
            messageId: currentBasis.messageId,
            viewportStartOffset: currentBasis.viewportStartOffset,
          },
          },
        });
      }
    } catch (err) {
      if (routeGenerationRef.current === routeGeneration && paginationRequestRef.current === requestGeneration) {
        setOlderError(err instanceof Error ? err.message : 'Failed to load earlier history.');
      }
    } finally {
      if (routeGenerationRef.current === routeGeneration && paginationRequestRef.current === requestGeneration) {
        if (pendingOlderRestoreRef.current === pendingRestore) pendingOlderRestoreRef.current = null;
        setLoadingOlder(false);
      }
    }
  }, [conversationStore, coordinator, historyGeneration, loadingOlder, productConversationId, snapshot]);

  const messages = aggregateMessages;
  const cleanupAttention = ordinarySnapshot?.close?.outcome === 'archived_cleanup_attention';
  const closeNeedsLiveUpdateOwner = closeIncomplete || cleanupAttention;
  const closeInProgress = ordinarySnapshot?.close != null && (ordinarySnapshot.close.phase !== 'completed' || closeIncomplete);
  const convState = useMemo(
    () => ordinarySnapshot?.ordinary_lifecycle === 'history' || closeInProgress
      ? ({ type: 'idle' } satisfies ConversationState)
      : currentMemberTranscript?.phase ?? (snapshot ? aggregateConversationState(snapshot, loadingOlder, olderError) : { type: 'idle' } satisfies ConversationState),
    [closeInProgress, currentMemberTranscript?.phase, loadingOlder, olderError, ordinarySnapshot?.ordinary_lifecycle, snapshot],
  );
  const latestSlug = snapshot?.latest_transcript_row_id ?? null;
  const latestConversationId = currentLatestProjection?.conversationId ?? snapshot?.latest_transcript_row_id ?? undefined;
  const latestWorkScopeKey = isOpen
    ? currentLatestProjection?.conversation?.work_scope_key
    : undefined;
  const transcriptView = useMemo(() => ({
    conversationId: snapshot?.product_conversation_id ?? '',
    generation: historyGeneration,
    transcriptGeneration: historyGeneration,
  }), [snapshot?.product_conversation_id, historyGeneration]);
  const routeReaderSelectionRef = useRef<{ pathname: string; productConversationId: string | undefined; transcriptPin: string | null; awaitingTopology: boolean } | null>(null);
  if (snapshot && (routeReaderSelectionRef.current?.pathname !== location.pathname
    || routeReaderSelectionRef.current.productConversationId !== productConversationId || routeReaderSelectionRef.current.awaitingTopology)) {
    const routeMemberId = snapshot.kind === 'coordinator'
      ? snapshot.segments.find(segment => segment.transcript_row_id === routeReference || segment.slug === routeReference)?.transcript_row_id
        ?? routeReference ?? snapshot.requested_transcript_row_id
      : null;
    routeReaderSelectionRef.current = {
      pathname: location.pathname,
      productConversationId,
      transcriptPin: routeMemberId !== snapshot.latest_transcript_row_id ? routeMemberId : null,
      awaitingTopology: routeMemberId !== null && !snapshot.segments.some(segment => segment.transcript_row_id === routeMemberId),
    };
  }
  const awaitingRouteTopology = routeReaderSelectionRef.current?.awaitingTopology ?? false;
  useEffect(() => {
    if (awaitingRouteTopology) setSnapshotRetry(value => value + 1);
  }, [awaitingRouteTopology, location.pathname]);
  const pendingChapterSelected = (location.state as { chapterSelection?: unknown } | null)?.chapterSelection === 'pending'
    && !hashTargetMessageId && !new URLSearchParams(location.search).has('source_transcript');
  const readerSelection = snapshot && !pendingChapterSelected
    ? transcriptReaderSelection(snapshot, location.search, routeReaderSelectionRef.current?.transcriptPin ?? null)
    : { kind: 'latest' as const };
  const selectedTranscriptMessage = !hashTargetMessageId && readerSelection.kind === 'transcript'
    ? messages.find((message) => message.conversation_id === readerSelection.segment.transcript_row_id)
    : undefined;
  const transcriptBoundary = snapshot && !hashTargetMessageId && readerSelection.kind === 'transcript' && !selectedTranscriptMessage
    ? representedOpeningBoundary(snapshot.boundaries, { kind: 'transcript', transcriptRowId: readerSelection.segment.transcript_row_id })
    : undefined;
  const transcriptTargetMessage = selectedTranscriptMessage ?? (transcriptBoundary
    ? messages.find(message => message.message_id === transcriptBoundary.message_id)
    : undefined);
  const transcriptTargetExhausted = !hashTargetMessageId && readerSelection.kind === 'transcript' && !transcriptTargetMessage && (
    snapshot?.has_older === false || (!transcriptBoundary && snapshot?.segments.some((segment) => segment.segment_ordinal < readerSelection.segment.segment_ordinal && segment.messages.length > 0))
  );
  const representedBoundary = snapshot && hashTargetMessageId
    ? representedOpeningBoundary(snapshot.boundaries, { kind: 'message', messageId: hashTargetMessageId })
    : undefined;
  const resolvedHashTargetMessageId = representedBoundary?.message_id ?? hashTargetMessageId;
  const hashTargetMessage = messages.find(message => message.message_id === resolvedHashTargetMessageId);
  const hashTargetTranscriptId = representedBoundary?.successor_transcript_row_id
    ?? hashTargetMessage?.conversation_id
    ?? snapshot?.boundaries.find(boundary => boundary.message_id === hashTargetMessageId)?.predecessor_transcript_row_id;
  const hashTargetSourceMismatch = readerSelection.kind === 'transcript' && hashTargetTranscriptId !== undefined
    && hashTargetTranscriptId !== readerSelection.segment.transcript_row_id;
  const hashTargetLoaded = hashTargetMessage !== undefined && !hashTargetSourceMismatch;
  const hashTargetExhausted = !!hashTargetMessageId && (hashTargetSourceMismatch || (!hashTargetLoaded && snapshot?.has_older === false));
  const positioningTargetMessageId = readerSelection.kind === 'unavailable' ? undefined : hashTargetLoaded ? resolvedHashTargetMessageId : transcriptTargetMessage?.message_id;
  const previousJump = desiredJumpRef.current;
  const completedJump = previousJump
    && previousJump.token === hashNavigation.token
    && previousJump.view.conversationId === transcriptView.conversationId
    && previousJump.targetMessageId === positioningTargetMessageId
    && jumpCompletion?.commandKey === transcriptPositioningInputCommandKey({ kind: 'positioning', command: previousJump });
  const desiredJump = useMemo(() => positioningTargetMessageId
    ? completedJump ? previousJump : {
      kind: 'jump_to_message' as const,
      token: hashNavigation.token,
      requestToken: 0,
      view: transcriptView,
      targetMessageId: positioningTargetMessageId,
    }
    : null, [positioningTargetMessageId, completedJump, previousJump, hashNavigation.token, transcriptView]);
  useLayoutEffect(() => { desiredJumpRef.current = desiredJump; }, [desiredJump]);
  const desiredPositioning: TranscriptPositioningInput = (restoreCommand?.navigationToken === hashNavigation.token ? restoreCommand.positioning : null)
    ?? (desiredJump ? { kind: 'positioning', command: desiredJump } : { kind: 'idle', view: transcriptView });
  const desiredPositioningKey = transcriptPositioningInputCommandKey(desiredPositioning);
  const transcriptPositioning: TranscriptPositioningInput = desiredPositioningKey !== null
    && jumpCompletion?.commandKey === desiredPositioningKey
    ? { kind: 'idle', view: transcriptView }
    : desiredPositioning;
  const liveControlsEnabled = isOpen && !closeInProgress && snapshot?.latest_opening.status !== 'pending';
  const goals = useConversationGoals(ordinarySnapshot?.product_conversation_id ?? null, liveControlsEnabled);
  const initialSnapshotReady = snapshot !== null;
  useLayoutEffect(() => {
    journey.expectTarget(!!hashTargetMessageId || readerSelection.kind !== 'latest' || awaitingRouteTopology, positioningTargetMessageId ?? null);
  }, [journey, hashTargetMessageId, readerSelection.kind, awaitingRouteTopology, positioningTargetMessageId]);
  const observedSegmentId = readerSelection.kind === 'transcript' ? readerSelection.segment.transcript_row_id : snapshot?.latest_transcript_row_id;
  useEffect(() => {
    if (snapshot && observedSegmentId) journey.hydrationReady(snapshot.product_conversation_id, observedSegmentId, messages.length);
    if (hashTargetExhausted || transcriptTargetExhausted || readerSelection.kind === 'unavailable') journey.anchor('target_missing');
  }, [journey, snapshot, observedSegmentId, messages.length, hashTargetExhausted, transcriptTargetExhausted, readerSelection.kind]);

  useEffect(() => {
    const measurement = openMeasurementRef.current;
    if (
      !measurement
      || measurement.reported
      || measurement.snapshotReceivedAt === undefined
      || !initialSnapshotReady
    ) return;
    measurement.storeReadyAt ??= performance.now();
    if (!measurement.initiallyVisible) {
      reportProductConversationOpen(measurement, null);
      return;
    }
    let reportFrame = 0;
    const paintFrame = requestAnimationFrame(() => {
      reportFrame = requestAnimationFrame((paintedAt) => {
        if (openMeasurementRef.current === measurement) {
          reportProductConversationOpen(measurement, paintedAt);
        }
      });
    });
    return () => {
      cancelAnimationFrame(paintFrame);
      cancelAnimationFrame(reportFrame);
    };
  }, [initialSnapshotReady, openSnapshotGeneration, productConversationId]);

  useEffect(() => {
    const needsMessage = !!hashTargetMessageId && !hashTargetLoaded && !hashTargetExhausted;
    const needsTranscript = !hashTargetMessageId && readerSelection.kind === 'transcript' && !transcriptTargetMessage && !transcriptTargetExhausted;
    if ((!needsMessage && !needsTranscript) || !snapshot?.has_older || loadingOlder || olderError || readerSelection.kind === 'unavailable') return;
    void loadOlderMessages();
  }, [hashTargetExhausted, hashTargetLoaded, hashTargetMessageId, loadOlderMessages, loadingOlder, olderError, readerSelection.kind, snapshot?.has_older, transcriptTargetExhausted, transcriptTargetMessage]);

  if (loading) {
    return (
      <main className="product-conversation-page" data-testid="product-conversation-page" ref={coordinator || inheritedJourney ? undefined : journey.attach}>
        <section id="chat-view" className="view active product-conversation-page__transcript">
          <div id="messages"><MessageListSkeleton count={6} /></div>
        </section>
      </main>
    );
  }

  if (!snapshot) {
    const fallbackSlug = productConversationId;
    return (
      <main className="product-conversation-page" ref={coordinator || inheritedJourney ? undefined : journey.attach}>
        <div role="alert">{error ?? 'Unable to open this product conversation.'} Showing cached row if available.</div>
        <button className="btn-secondary" type="button" onClick={() => setSnapshotRetry((retry) => retry + 1)}>Retry</button>
        {fallbackSlug && (
          <EmbeddedConversationPage
            slug={fallbackSlug}
            routePrefix={coordinator ? "/global" : "/c"}
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
      ref={coordinator || inheritedJourney ? undefined : journey.attach}
    >
      {ordinarySnapshot && <ProductConversationHeader
        snapshot={ordinarySnapshot}
        productConversationId={snapshot.product_conversation_id}
        messages={messages}
        recallDisabled={!liveControlsEnabled}
      />}
      {ordinarySnapshot && !liveTranscriptRowId ? <details>
        <summary><ConversationGoalSummary model={goals} /></summary>
        <ConversationGoalDetails key={ordinarySnapshot.product_conversation_id} model={goals} />
      </details> : null}
      {ordinarySnapshot?.close && (ordinarySnapshot.close.outcome === 'archived_cleanup_attention' || closeIncomplete) && (
        <section className="product-conversation-page__close-status" role="alert" aria-label="Close status">
          <strong>{closeIncomplete
            ? 'Close incomplete — shutdown could not be confirmed'
            : 'Closed — cleanup needs attention'}</strong>
          <div>Run {ordinarySnapshot.close.run_ordinal} · {ordinarySnapshot.close.run_status}. No automatic retry.</div>
          {ordinarySnapshot.close.failure && (
            <>
              <div>{ordinarySnapshot.close.failure.reason.replaceAll('_', ' ')} — {ordinarySnapshot.close.failure.detail}</div>
              <div>{ordinarySnapshot.close.failure.stop_certainty.kind === 'conversation_and_processes_stopped'
                ? 'Conversation and processes stopped.' : 'Shutdown uncertain.'}</div>
              <div>Failure <code>{ordinarySnapshot.close.failure.occurrence_id}</code></div>
              <details>
                <summary>Remaining resources ({ordinarySnapshot.close.failure.remaining_resources.length})</summary>
                <ol>
                  {ordinarySnapshot.close.failure.remaining_resources.map((resource, ordinal) => (
                    <li key={ordinal}>
                      <code>{resource.scope}: {resource.resource_kind.replaceAll('_', ' ')} · {resource.identity}</code>
                      {' · '}{resource.disposition}
                    </li>
                  ))}
                </ol>
              </details>
            </>
          )}
        </section>
      )}
      {(readerSelection.kind === 'unavailable' || transcriptTargetExhausted) && (
        <div className="error-message" role="alert">Original source transcript unavailable.</div>
      )}
      {(olderError || error || hashTargetExhausted) && (
        <div className="product-conversation-page__status" role="alert">
          {olderError ?? error ?? 'The linked message is not available in this conversation history.'}
        </div>
      )}
      <section className="view active product-conversation-page__transcript" data-testid="product-conversation-transcript">
        <ConversationNavStack
          messages={messages}
          onViewChaptersReady={journey.chaptersReady}
          onViewTailSettled={journey.tailAnchorSettled}
          onViewAnchorProbe={journey.registerAnchorProbe}
          viewAnchorIntent={{
            scope: `${hashNavigation.token}:${observedSegmentId ?? ''}:${historyGeneration}`,
            ...(desiredJump ? { command: desiredJump } : { command: null, view: transcriptView }),
          }}
          historyBoundaries={snapshot.boundaries}
          streamingRequestId={closeInProgress ? null : currentMemberTranscript?.streamingRequestId ?? null}
          pendingMessages={aggregatePendingMessages}
          convState={convState}
          onRetry={liveControlsEnabled ? currentLatestProjection?.onRetryPending ?? (() => {}) : () => {}}
          onCancelSteering={liveControlsEnabled ? currentLatestProjection?.onCancelSteering : undefined}
          onOpenFile={currentLatestProjection?.onOpenFile}
          filePathRootDir={currentLatestProjection?.filePathRootDir}
          systemPrompt={currentLatestProjection?.systemPrompt}
          enableMessageSidepanel
          enableMessageFullscreen
          reactionScopeKey={snapshot.product_conversation_id}
          reactionDestination={liveControlsEnabled && currentLatestProjection?.appendInlineReactionToComposer
            ? { append: currentLatestProjection.appendInlineReactionToComposer }
            : undefined}
          conversationId={snapshot.product_conversation_id}
          slug={latestSlug ?? snapshot.canonical_root.slug ?? snapshot.requested_transcript_row_id}
          hasOlderMessages={snapshot.has_older}
          onLoadOlderMessages={snapshot.has_older ? loadOlderMessages : undefined}
          onUpdateOlderMessagesRestore={updateOlderMessagesRestore}
          loadingOlderMessages={loadingOlder}
          olderHistoryError={olderError}
          transcriptPositioning={transcriptPositioning}
          onHistoryScrollCommandHandled={(completion) => {
            if (completion.commandKey !== desiredPositioningKey) return;
            journey.anchor(completion.result);
            if (desiredPositioning.kind === 'positioning' && desiredPositioning.command.kind === 'jump_to_message') {
              setJumpCompletion(completion);
            }
            setRestoreCommand((current) => (
              current && transcriptPositioningInputCommandKey(current.positioning) === completion.commandKey ? null : current
            ));
          }}
          {...(latestWorkScopeKey ? { workScopeKey: latestWorkScopeKey } : {})}
        />
      </section>
      {transcriptAuthority?.kind === 'awaiting_init' && (
        <RetryTranscriptHydrationOwner
          key={snapshot.latest_transcript_row_id}
          conversationId={snapshot.latest_transcript_row_id}
        />
      )}
      {liveTranscriptRowId && (
        <div
          className="product-conversation-page__composer"
          data-testid="product-conversation-composer"
        >
          <EmbeddedConversationPage
            slug={liveTranscriptRowId}
            showTranscript={false}
            embeddedShell
            suppressCanonicalization={true}
            suppressMessageViewerOwner
            suppressTaskApprovalOwner={true}
            mutationEnabled={liveControlsEnabled}
            aggregateLifecycleOpen={isOpen}
            onProjectionChange={setLatestProjection}
            onStreamAttempt={journey.streamStarted}
            routePrefix={coordinator ? "/global" : "/c"}
            composerQuickAction={globalShell?.composerQuickAction}
            stateBarExtension={ordinarySnapshot ? {
              summary: <ConversationGoalSummary model={goals} />,
              details: <ConversationGoalDetails key={ordinarySnapshot.product_conversation_id} model={goals} />,
            } : globalShell?.stateBarExtension}
            onCloseCompleted={() => setSnapshotRetry((retry) => retry + 1)}
          />
        </div>
      )}
      {closeNeedsLiveUpdateOwner && snapshot.latest_transcript_row_id && (
        <div data-testid="product-conversation-close-live-update-owner" hidden>
          <CloseLiveUpdateOwner conversationId={snapshot.latest_transcript_row_id} />
        </div>
      )}
      {(!isOpen || closeIncomplete) && (
        <div className="product-conversation-page__composer-placeholder" data-testid={closeIncomplete ? 'product-conversation-close-incomplete' : 'product-conversation-history'}>
          {closeIncomplete ? 'Conversation controls are disabled until shutdown is resolved.' : 'History is read-only.'}
        </div>
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
                onSendNotes={liveControlsEnabled && currentLatestProjection?.appendReviewNotesToComposer
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

        {taskApprovalOverlay && currentLatestProjection?.conversationId === taskApprovalOverlay.conversationId && (
          <Suspense fallback={null}>
            <TaskApprovalReader
              title={taskApprovalOverlay.title}
              priority={taskApprovalOverlay.priority}
              plan={taskApprovalOverlay.plan}
              contextWindowUsed={approvalContextWindowUsed ?? undefined}
              modelContextWindow={currentLatestProjection.modelContextWindow}
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
