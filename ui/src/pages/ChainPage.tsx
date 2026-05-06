// Chain page (REQ-CHN-003 / 004 / 005 / 006 / 007).
//
// Two-column layout: members on the left, Q&A panel (a vertical scratchpad of
// pair cards) on the right. SSE subscription is opened on mount and closed on
// unmount; tokens are demuxed by `chain_qa_id` so a sibling tab's question on
// the same chain does not bleed into this tab's render.
//
// Layout (REQ-CHN-005). The Q&A panel is a top-down list of "pair cards." The
// active pair card sits at index 0 — its Q row is an autofocused textarea, its
// A row is a placeholder. On submit, the just-submitted pair drops into the
// list at index 1 (newest in-flight just below the active card) and the active
// card refocuses for the next question. Persisted pairs render in reverse
// chronological order below in-flight pairs.
//
// Optimistic submit (task 08682). The submit flow does NOT block on the POST:
//   1. Mint a client-side temp id (`crypto.randomUUID()`).
//   2. Dispatch `OPTIMISTIC_INFLIGHT_ADD` synchronously — the in-flight pair
//      card appears, the draft clears, the textarea refocuses for the next
//      question, all before the POST returns.
//   3. POST `submitChainQuestion` in the background.
//   4. On success, `INFLIGHT_RECONCILE_ID` rekeys the entry from the temp id
//      to the server-issued real id so subsequent SSE token events find it.
//   5. On failure, `INFLIGHT_DROP` removes the optimistic entry and
//      `SUBMIT_FAIL` surfaces the error; the draft is preserved so the user
//      can retry without retyping.
//
// REQ-CHN-006 visual independence: each Q&A entry renders as a self-contained
// pair card with explicit `Q:` and `A:` rows — no chat-style ligatures, no
// thread/reply lines. The active card has the same shape as past pairs (just
// unfilled), which structurally communicates that the next question creates a
// new pair rather than continuing a thread.

import { useEffect, useMemo, useRef, useState, useCallback } from 'react';
import type { FormEvent, KeyboardEvent } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import {
  api,
  subscribeToChainStream,
  type ChainView,
  type ChainQaRow,
  type ChainMemberSummary,
  type ChainSseEventData,
} from '../api';
import { ChainDeleteConfirm } from '../components/ChainDeleteConfirm';
import { useChainAtom, type InflightQa } from '../chain';

// Markdown plugin set, hoisted so the array identity is stable across
// renders (matches the pattern in StreamingMessage.tsx).
const REMARK_PLUGINS = [remarkGfm];
import { formatShortDateTime } from '../utils';

export function ChainPage() {
  const { rootConvId } = useParams<{ rootConvId: string }>();
  const navigate = useNavigate();

  // ---------------------------------------------------------------------------
  // Chain-scoped state lives in ChainStore (task 08682). Migrating off plain
  // useState gave us:
  //   - dead-atom protection on navigation (an in-flight `getChain(A)` that
  //     resolves after navigating to chain B writes into atom A, not into
  //     chain B's component state),
  //   - per-key state by construction so we no longer need a synchronous
  //     reset block on rootConvId change,
  //   - a clean place for true optimistic submit (the dispatch is
  //     synchronous; the POST round-trip happens in the background).
  // ---------------------------------------------------------------------------
  const [atom, dispatch] = useChainAtom(rootConvId ?? null);
  const { chain, loadError, loading, inflight, inflightOrder, draft, submitting, sseLost } = atom;

  // Component-local: the delete-confirm modal is a per-render-instance UI
  // affordance, not chain state. It does not need to survive navigation
  // (in fact: it should *not* — dialog open across nav would be a bug).
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);

  // Imperative handle to the active pair's textarea so we can refocus it
  // immediately after submit (the user agreed they should be able to type the
  // next question without waiting for the answer).
  const activeTextareaRef = useRef<HTMLTextAreaElement | null>(null);

  // Live mirror of `atom.inflight` for the SSE error handler. The error
  // handler closes over render-time state, but the user can submit a new
  // question (which expands `inflight`) AFTER the SSE effect mounted.
  // Without this ref, `handleErr` would see the stale empty inflight from
  // mount time and fail to dispatch SSE_LOST in exactly the scenario the
  // affordance is meant to cover. (Codex review on PR #26.)
  const inflightRef = useRef(atom.inflight);
  inflightRef.current = atom.inflight;

  /** Refresh the chain snapshot. Used after submit/complete/fail. The atom
   *  routing (08682) makes this safe across navigation: an in-flight
   *  `getChain(rootId)` that resolves after we navigate elsewhere
   *  dispatches LOAD_OK against atom `rootId`, not against whichever atom
   *  is currently active.
   *
   *  Network failure does not blow the page away if we already have a
   *  snapshot — the reducer's LOAD_FAIL preserves `chain`. */
  const refresh = useCallback(
    async (rootId: string) => {
      try {
        const view = await api.getChain(rootId);
        dispatch({ type: 'LOAD_OK', view });
        return view;
      } catch (err) {
        const msg = err instanceof Error ? err.message : 'Failed to load chain';
        dispatch({ type: 'LOAD_FAIL', error: msg });
        return null;
      }
    },
    [dispatch],
  );

  // Initial load.
  useEffect(() => {
    if (!rootConvId) return;
    dispatch({ type: 'LOAD_BEGIN' });
    let cancelled = false;
    api
      .getChain(rootConvId)
      .then((view) => {
        if (cancelled) return;
        dispatch({ type: 'LOAD_OK', view });
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        const msg = err instanceof Error ? err.message : 'Failed to load chain';
        dispatch({ type: 'LOAD_FAIL', error: msg });
      });
    return () => {
      cancelled = true;
    };
  }, [rootConvId, dispatch]);

  // SSE subscription. We open one EventSource for the chain and demux events
  // by chain_qa_id against the atom's in-flight buffer. Close on unmount or
  // root change so leaving the page tears down the connection cleanly.
  useEffect(() => {
    if (!rootConvId) return;

    const handleEvent = (evt: ChainSseEventData) => {
      // We only render tokens for in-flight Q&As we know about (i.e., ours).
      // Sibling-tab Q&A on the same chain falls through to a refetch when it
      // completes — but its tokens never bleed into our buffer.
      if (evt.type === 'chain_qa_token') {
        dispatch({
          type: 'TOKEN_APPENDED',
          chainQaId: evt.chain_qa_id,
          delta: evt.delta,
        });
      } else if (evt.type === 'chain_qa_completed') {
        // Drop the in-flight entry and refetch to pick up the persisted row.
        dispatch({ type: 'INFLIGHT_DROP', chainQaId: evt.chain_qa_id });
        // Refetch so the persisted ChainQaRow replaces the optimistic entry.
        // Even for sibling-tab questions, refetching is the simplest way to
        // surface their newly-persisted answer.
        void refresh(rootConvId);
      } else if (evt.type === 'chain_qa_failed') {
        dispatch({
          type: 'INFLIGHT_FAIL',
          chainQaId: evt.chain_qa_id,
          error: evt.error,
          partialAnswer: evt.partial_answer ?? null,
        });
        // Refetch: server has persisted the failed row with `status=failed`,
        // so on next render we let the persisted row drive UI and drop our
        // optimistic entry.
        void refresh(rootConvId).then(() => {
          dispatch({ type: 'INFLIGHT_DROP', chainQaId: evt.chain_qa_id });
        });
      }
    };

    const handleErr = () => {
      // EventSource will auto-reconnect on its own. We surface a "connection
      // lost" affordance only if there is in-flight work that depends on it.
      // Read inflight via a ref so the live state is observed — a question
      // submitted after this effect ran would not appear in the render-time
      // closure of `atom.inflight`.
      if (Object.keys(inflightRef.current).length > 0) {
        dispatch({ type: 'SSE_LOST' });
      }
    };

    const es = subscribeToChainStream(rootConvId, handleEvent, handleErr);
    dispatch({ type: 'SSE_RESTORED' });
    return () => es.close();
  }, [rootConvId, dispatch, refresh]);

  /**
   * Optimistic submit (task 08682 acceptance criterion).
   *
   * The dispatch happens synchronously before the POST so the in-flight
   * pair card is in the DOM and the draft is cleared *immediately* on
   * click. The textarea stays enabled across the round-trip; the
   * `submitting` flag drives only the Ask button label and not the
   * textarea's disabled prop. When the POST returns the real chain_qa_id,
   * we rekey the optimistic entry so subsequent SSE tokens find it.
   */
  const submit = useCallback(
    async (question: string) => {
      if (!rootConvId || !chain) return;
      const trimmed = question.trim();
      if (!trimmed) return;

      const tempId = `temp-${crypto.randomUUID()}`;
      // Synchronous: card appears + draft clears in the next render.
      dispatch({ type: 'OPTIMISTIC_INFLIGHT_ADD', chainQaId: tempId, question: trimmed });
      dispatch({ type: 'SUBMIT_BEGIN' });
      // Refocus the active textarea immediately so the user can type the next
      // question while the POST is in flight. Defer to a microtask so React
      // has a chance to flush the value reset before we steal focus back.
      queueMicrotask(() => {
        activeTextareaRef.current?.focus();
      });

      try {
        const { chain_qa_id } = await api.submitChainQuestion(rootConvId, trimmed);
        // Rekey the optimistic entry to the server's real id. Subsequent
        // SSE token events keyed on chain_qa_id will now find it.
        dispatch({ type: 'INFLIGHT_RECONCILE_ID', tempId, realId: chain_qa_id });
        dispatch({ type: 'SUBMIT_OK' });
      } catch (err) {
        const msg = err instanceof Error ? err.message : 'Failed to submit question';
        // Drop the optimistic card and surface the error. The draft was
        // already cleared in OPTIMISTIC_INFLIGHT_ADD; restore it so the
        // user can retry without retyping.
        dispatch({ type: 'INFLIGHT_DROP', chainQaId: tempId });
        dispatch({ type: 'DRAFT_SET', value: trimmed });
        dispatch({ type: 'SUBMIT_FAIL', error: msg });
      }
    },
    [rootConvId, chain, dispatch],
  );

  const handleSubmit = (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    void submit(draft);
  };

  /** Re-ask: populate the active textarea with the original question and
   *  focus it. Do NOT auto-submit — REQ-CHN-007's editing pattern preserves
   *  user agency, and consistency with that precedent matters here. */
  const handleReask = (question: string) => {
    dispatch({ type: 'DRAFT_SET', value: question });
    queueMicrotask(() => {
      activeTextareaRef.current?.focus();
    });
  };

  const setDraft = useCallback(
    (value: string) => dispatch({ type: 'DRAFT_CHANGED', value }),
    [dispatch],
  );

  // Persisted rows in chronological order (oldest first). We reverse on render
  // so the newest persisted card sits just below in-flight cards. In-flight
  // entries also render newest-first; both kinds of cards stack below the
  // active card at index 0.
  const renderableQas = useMemo(() => {
    const persisted: ChainQaRow[] = chain?.qa_history.slice() ?? [];
    persisted.sort((a, b) => (a.created_at < b.created_at ? -1 : 1));
    const inflightList: InflightQa[] = inflightOrder
      .map((id) => inflight[id])
      .filter((entry): entry is InflightQa => entry !== undefined);
    return { persisted, inflightList };
  }, [chain, inflight, inflightOrder]);

  // Loading state — show a lightweight placeholder. Errors take over the
  // whole page (404 → empty state, generic error → error message + retry).
  if (loading && !chain) {
    return (
      <div className="chain-page chain-page--loading">
        <div className="chain-page-empty">Loading chain…</div>
      </div>
    );
  }

  if (loadError && !chain) {
    const isNotFound = /not found/i.test(loadError);
    return (
      <div className="chain-page chain-page--empty">
        <div className="chain-page-empty">
          <h2>{isNotFound ? 'Not a chain' : 'Could not load chain'}</h2>
          <p>
            {isNotFound
              ? 'This conversation is not the root of a chain. Chains require at least two conversations linked by continuation.'
              : loadError}
          </p>
          <button
            type="button"
            className="btn-primary"
            onClick={() => navigate('/')}
          >
            Back to conversations
          </button>
        </div>
      </div>
    );
  }

  if (!chain) {
    return null;
  }

  return (
    <div className="chain-page">
      <ChainPageHeader
        chain={chain}
        onRename={async (name) => {
          if (!rootConvId) return;
          try {
            const updated = await api.setChainName(rootConvId, name);
            dispatch({ type: 'LOAD_OK', view: updated });
          } catch (err) {
            dispatch({
              type: 'LOAD_FAIL',
              error: err instanceof Error ? err.message : 'Failed to rename chain',
            });
          }
        }}
        onArchiveToggle={async () => {
          if (!rootConvId) return;
          try {
            if (chain.archived) {
              await api.unarchiveChain(rootConvId);
            } else {
              await api.archiveChain(rootConvId);
            }
            navigate('/');
          } catch (err) {
            dispatch({
              type: 'LOAD_FAIL',
              error:
                err instanceof Error
                  ? err.message
                  : chain.archived
                    ? 'Failed to unarchive chain'
                    : 'Failed to archive chain',
            });
          }
        }}
        onDelete={() => setDeleteConfirmOpen(true)}
      />
      <div className="chain-page-body">
        <ChainMembersColumn
          members={chain.members}
          onMemberClick={(member) => {
            if (member.slug) {
              navigate(`/c/${member.slug}`);
            }
          }}
        />
        <ChainQaColumn
          chain={chain}
          persisted={renderableQas.persisted}
          inflight={renderableQas.inflightList}
          draft={draft}
          setDraft={setDraft}
          submitting={submitting}
          sseLost={sseLost}
          onSubmit={handleSubmit}
          onReask={handleReask}
          activeTextareaRef={activeTextareaRef}
          onRetryConnection={() => {
            // Force the SSE effect to re-run by re-fetching first; the
            // EventSource itself will already be auto-reconnecting under the
            // hood, but this gives the user a clear "I tried" affordance.
            dispatch({ type: 'SSE_RESTORED' });
            if (rootConvId) void refresh(rootConvId);
          }}
        />
      </div>
      <ChainDeleteConfirm
        visible={deleteConfirmOpen}
        chain={chain}
        onConfirm={async () => {
          if (!rootConvId) return;
          try {
            await api.deleteChain(rootConvId);
            setDeleteConfirmOpen(false);
            navigate('/');
          } catch (err) {
            dispatch({
              type: 'LOAD_FAIL',
              error: err instanceof Error ? err.message : 'Failed to delete chain',
            });
            setDeleteConfirmOpen(false);
          }
        }}
        onCancel={() => setDeleteConfirmOpen(false)}
      />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Header (REQ-CHN-007 inline name edit)
// ---------------------------------------------------------------------------

interface ChainPageHeaderProps {
  chain: ChainView;
  onRename: (name: string | null) => Promise<void>;
  /** Toggles the chain's archived state — Archive when chain.archived is
   *  false, Unarchive when true. The parent picks which API call to fire. */
  onArchiveToggle: () => void | Promise<void>;
  onDelete: () => void;
}

function ChainPageHeader({ chain, onRename, onArchiveToggle, onDelete }: ChainPageHeaderProps) {
  const [editing, setEditing] = useState(false);
  // The text input is pre-populated with the actual override (`chain_name`),
  // not the resolved `display_name` — REQ-CHN-007 spec note: an empty input
  // means "clear the override and fall back to title."
  const [value, setValue] = useState(chain.chain_name ?? '');
  const inputRef = useRef<HTMLInputElement | null>(null);

  // Keep the local value in sync if the prop changes while we're not editing
  // (e.g., after a successful PATCH refresh).
  useEffect(() => {
    if (!editing) setValue(chain.chain_name ?? '');
  }, [chain.chain_name, editing]);

  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  const commit = async () => {
    const trimmed = value.trim();
    // Mirror the server's null-on-empty rule client-side so we don't round-trip
    // an empty string just to have it normalized on the other end.
    const next: string | null = trimmed.length === 0 ? null : trimmed;
    if (next === (chain.chain_name ?? null)) {
      setEditing(false);
      return;
    }
    await onRename(next);
    setEditing(false);
  };

  const cancel = () => {
    setValue(chain.chain_name ?? '');
    setEditing(false);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      void commit();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      cancel();
    }
  };

  return (
    <header className="chain-page-header">
      {editing ? (
        <input
          ref={inputRef}
          className="chain-page-name-input"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onBlur={() => void commit()}
          onKeyDown={onKeyDown}
          aria-label="Chain name"
          placeholder="Name this chain…"
          maxLength={200}
        />
      ) : (
        <button
          type="button"
          className="chain-page-name"
          onClick={() => setEditing(true)}
          title="Click to rename chain"
        >
          {chain.display_name}
        </button>
      )}
      <span className="chain-page-meta">
        {chain.current_member_count}{' '}
        {chain.current_member_count === 1 ? 'conversation' : 'conversations'}
        {' · '}
        {chain.current_total_messages} messages
      </span>
      <div className="chain-page-actions">
        <button
          type="button"
          className="btn-secondary"
          onClick={() => void onArchiveToggle()}
        >
          {chain.archived ? 'Unarchive' : 'Archive'}
        </button>
        <button
          type="button"
          className="btn-danger"
          onClick={onDelete}
        >
          Delete
        </button>
      </div>
    </header>
  );
}

// ---------------------------------------------------------------------------
// Members column
// ---------------------------------------------------------------------------

interface ChainMembersColumnProps {
  members: ChainMemberSummary[];
  onMemberClick: (member: ChainMemberSummary) => void;
}

function ChainMembersColumn({ members, onMemberClick }: ChainMembersColumnProps) {
  return (
    <aside className="chain-members" aria-label="Chain members">
      <h3 className="chain-members-heading">Conversations</h3>
      <ol className="chain-members-list">
        {members.map((m) => {
          const label = positionLabel(m.position);
          const isLatest = m.position === 'latest';
          return (
            <li
              key={m.conv_id}
              className={`chain-member ${isLatest ? 'chain-member--latest' : ''}`}
            >
              <button
                type="button"
                className="chain-member-card"
                onClick={() => onMemberClick(m)}
                disabled={!m.slug}
                title={m.slug ? `Open ${m.slug}` : undefined}
              >
                <div className="chain-member-row1">
                  <span className="chain-member-title">
                    {m.title ?? m.slug ?? m.conv_id}
                  </span>
                  {isLatest && (
                    <span className="chain-member-badge">Latest</span>
                  )}
                </div>
                <div className="chain-member-row2">
                  <span className="chain-member-position">{label}</span>
                  <span className="chain-member-sep">·</span>
                  <span className="chain-member-count">
                    {m.message_count} msg
                  </span>
                  <span className="chain-member-sep">·</span>
                  <span className="chain-member-date">
                    {formatShortDateTime(m.updated_at)}
                  </span>
                </div>
              </button>
            </li>
          );
        })}
      </ol>
    </aside>
  );
}

function positionLabel(p: ChainMemberSummary['position']): string {
  switch (p) {
    case 'root':
      return 'Root';
    case 'continuation':
      return 'Continuation';
    case 'latest':
      return 'Latest';
  }
}

// ---------------------------------------------------------------------------
// Q&A column — scratchpad of pair cards
// ---------------------------------------------------------------------------

interface ChainQaColumnProps {
  chain: ChainView;
  persisted: ChainQaRow[];
  inflight: InflightQa[];
  draft: string;
  setDraft: (s: string) => void;
  submitting: boolean;
  sseLost: boolean;
  onSubmit: (e: FormEvent<HTMLFormElement>) => void;
  onReask: (question: string) => void;
  activeTextareaRef: React.RefObject<HTMLTextAreaElement>;
  onRetryConnection: () => void;
}

function ChainQaColumn({
  chain,
  persisted,
  inflight,
  draft,
  setDraft,
  submitting,
  sseLost,
  onSubmit,
  onReask,
  activeTextareaRef,
  onRetryConnection,
}: ChainQaColumnProps) {
  return (
    <section className="chain-qa" aria-label="Chain questions and answers">
      <div className="chain-qa-scroll">
        <ul className="chain-qa-list">
          <li>
            <ChainQaPairCard
              variant="active"
              draft={draft}
              setDraft={setDraft}
              submitting={submitting}
              onSubmit={onSubmit}
              activeTextareaRef={activeTextareaRef}
            />
          </li>
          {[...inflight].reverse().map((entry) => (
            <li key={entry.chainQaId}>
              <ChainQaPairCard
                variant={entry.error ? 'inflight-failed' : 'inflight-streaming'}
                inflightEntry={entry}
                onReask={onReask}
              />
            </li>
          ))}
          {[...persisted].reverse().map((row) => (
            <li key={row.id}>
              <ChainQaPairCard
                variant="persisted"
                row={row}
                chain={chain}
                onReask={onReask}
              />
            </li>
          ))}
        </ul>
      </div>
      {sseLost && (
        <div className="chain-qa-sse-lost" role="status">
          <span>Connection lost — tokens may be delayed.</span>
          <button type="button" className="btn-link" onClick={onRetryConnection}>
            Retry
          </button>
        </div>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// Pair card — one component, multiple visual variants (REQ-CHN-005, -006)
// ---------------------------------------------------------------------------
//
// A pair card always renders two labeled rows: `Q:` and `A:`. The variants
// differ only in what fills those rows.
//
//   active             — Q row = autofocused textarea + Ask button
//                        A row = "waiting for question" placeholder
//   inflight-streaming — Q row = static text
//                        A row = streaming markdown + blinking cursor
//   inflight-failed    — Q row = static text
//                        A row = partial markdown + "Failed: <error>" + Re-ask
//   persisted          — Q row = static text
//                        A row = depends on row.status:
//                          completed — markdown answer + staleness tag
//                          in_flight — sibling-tab "Still working…" placeholder
//                          failed    — partial markdown + "Failed" + Re-ask
//                          abandoned — "Did not complete" + Re-ask

type PairVariant =
  | 'active'
  | 'inflight-streaming'
  | 'inflight-failed'
  | 'persisted';

interface ChainQaPairCardProps {
  variant: PairVariant;
  // active variant
  draft?: string;
  setDraft?: (s: string) => void;
  submitting?: boolean;
  onSubmit?: (e: FormEvent<HTMLFormElement>) => void;
  activeTextareaRef?: React.RefObject<HTMLTextAreaElement>;
  // inflight variants
  inflightEntry?: InflightQa;
  // persisted variant
  row?: ChainQaRow;
  chain?: ChainView;
  // shared (inflight-failed, persisted)
  onReask?: (question: string) => void;
}

function ChainQaPairCard(props: ChainQaPairCardProps) {
  if (props.variant === 'active') {
    return <ActivePairCard {...props} />;
  }
  if (props.variant === 'inflight-streaming' || props.variant === 'inflight-failed') {
    return <InflightPairCard {...props} />;
  }
  return <PersistedPairCard {...props} />;
}

function ActivePairCard({
  draft = '',
  setDraft = () => {},
  submitting = false,
  onSubmit = () => {},
  activeTextareaRef,
}: ChainQaPairCardProps) {
  // Autofocus on mount. The ref is also used by the parent to refocus after
  // submit (which doesn't unmount this component — same node, just cleared).
  useEffect(() => {
    activeTextareaRef?.current?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const canSubmit = draft.trim().length > 0 && !submitting;

  return (
    <article className="chain-qa-pair chain-qa-pair--active">
      <form className="chain-qa-pair-form" onSubmit={onSubmit}>
        <div className="chain-qa-pair-row">
          <span className="chain-qa-pair-label">Q:</span>
          <div className="chain-qa-pair-content">
            <textarea
              ref={activeTextareaRef}
              className="chain-qa-input"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault();
                  if (canSubmit) {
                    // Single submit path — mirror the form submit.
                    onSubmit({
                      preventDefault: () => {},
                    } as unknown as FormEvent<HTMLFormElement>);
                  }
                }
              }}
              placeholder="Ask the chain a question…"
              rows={2}
              // Task 08682: textarea is never disabled mid-keystroke. The
              // submit flow is now optimistic; the user can type their
              // next question while a previous one is still POST-ing.
              aria-label="Question"
            />
          </div>
        </div>
        <div className="chain-qa-pair-row">
          <span className="chain-qa-pair-label">A:</span>
          <div className="chain-qa-pair-content chain-qa-pair-content--placeholder">
            <em>waiting for question</em>
          </div>
        </div>
        <div className="chain-qa-pair-actions">
          <button
            type="submit"
            className="btn-primary chain-qa-ask"
            disabled={!canSubmit}
          >
            {submitting ? 'Sending…' : 'Ask'}
          </button>
        </div>
      </form>
    </article>
  );
}

function InflightPairCard({ inflightEntry, onReask }: ChainQaPairCardProps) {
  if (!inflightEntry) return null;
  const isFailed = inflightEntry.error !== null;
  const className = isFailed
    ? 'chain-qa-pair chain-qa-pair--failed'
    : 'chain-qa-pair chain-qa-pair--streaming';
  return (
    <article className={className}>
      <div className="chain-qa-pair-row">
        <span className="chain-qa-pair-label">Q:</span>
        <div className="chain-qa-pair-content chain-qa-pair-question">
          {inflightEntry.question}
        </div>
      </div>
      <div className="chain-qa-pair-row">
        <span className="chain-qa-pair-label">A:</span>
        <div className="chain-qa-pair-content">
          {isFailed ? (
            <div className="chain-qa-answer chain-qa-answer--failed">
              {inflightEntry.answer.length > 0 && (
                <div className="chain-qa-partial chain-qa-markdown">
                  <ReactMarkdown remarkPlugins={REMARK_PLUGINS}>
                    {inflightEntry.answer}
                  </ReactMarkdown>
                </div>
              )}
              <div className="chain-qa-failure">
                <span className="chain-qa-failure-label">
                  Failed: {inflightEntry.error}
                </span>
                <button
                  type="button"
                  className="btn-link"
                  onClick={() => onReask?.(inflightEntry.question)}
                >
                  Re-ask
                </button>
              </div>
            </div>
          ) : inflightEntry.preToken ? (
            <div
              className="chain-qa-answer chain-qa-answer--skeleton"
              aria-live="polite"
            >
              <span className="chain-qa-skeleton-line" />
              <span className="chain-qa-skeleton-line" />
              <span className="chain-qa-skeleton-line chain-qa-skeleton-line--short" />
            </div>
          ) : (
            <div
              className="chain-qa-answer chain-qa-answer--streaming chain-qa-markdown"
              aria-live="polite"
            >
              <ReactMarkdown remarkPlugins={REMARK_PLUGINS}>
                {inflightEntry.answer}
              </ReactMarkdown>
              <span className="chain-qa-cursor" aria-hidden="true">
                ▍
              </span>
            </div>
          )}
        </div>
      </div>
    </article>
  );
}

function PersistedPairCard({ row, chain, onReask }: ChainQaPairCardProps) {
  if (!row || !chain) return null;
  const stale = stalenessLabel(row, chain);
  return (
    <article className={`chain-qa-pair chain-qa-pair--${row.status}`}>
      <div className="chain-qa-pair-row">
        <span className="chain-qa-pair-label">Q:</span>
        <div className="chain-qa-pair-content chain-qa-pair-question">
          {row.question}
        </div>
      </div>
      <div className="chain-qa-pair-row">
        <span className="chain-qa-pair-label">A:</span>
        <div className="chain-qa-pair-content">
          {renderPersistedAnswer(row, onReask)}
        </div>
      </div>
      <div className="chain-qa-meta">
        <time dateTime={row.created_at}>{formatShortDateTime(row.created_at)}</time>
        {stale && <span className="chain-qa-stale">{stale}</span>}
      </div>
    </article>
  );
}

function renderPersistedAnswer(
  row: ChainQaRow,
  onReask: ((q: string) => void) | undefined,
): JSX.Element {
  if (row.status === 'completed') {
    return (
      <div className="chain-qa-answer chain-qa-markdown">
        <ReactMarkdown remarkPlugins={REMARK_PLUGINS}>
          {row.answer ?? ''}
        </ReactMarkdown>
      </div>
    );
  }
  if (row.status === 'in_flight') {
    // Sibling-tab streaming case (REQ-CHN-005 "still working…" placeholder).
    // Our own in-flight Q&As render via InflightPairCard, not this function —
    // so reaching here means another subscriber is currently generating this.
    return (
      <div className="chain-qa-answer chain-qa-answer--placeholder">
        <em>Still working…</em>
      </div>
    );
  }
  if (row.status === 'failed') {
    return (
      <div className="chain-qa-answer chain-qa-answer--failed">
        {row.answer && row.answer.length > 0 ? (
          <div className="chain-qa-partial chain-qa-markdown">
            <ReactMarkdown remarkPlugins={REMARK_PLUGINS}>
              {row.answer}
            </ReactMarkdown>
          </div>
        ) : null}
        <div className="chain-qa-failure">
          <span className="chain-qa-failure-label">Failed</span>
          <button
            type="button"
            className="btn-link"
            onClick={() => onReask?.(row.question)}
          >
            Re-ask
          </button>
        </div>
      </div>
    );
  }
  // abandoned
  return (
    <div className="chain-qa-answer chain-qa-answer--abandoned">
      <div className="chain-qa-failure">
        <span className="chain-qa-failure-label">Did not complete</span>
        <button
          type="button"
          className="btn-link"
          onClick={() => onReask?.(row.question)}
        >
          Re-ask
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Snapshot staleness (REQ-CHN-005)
// ---------------------------------------------------------------------------

/** Format the inline staleness tag for a Q&A row. Returns null when the
 *  snapshot matches current chain state. */
function stalenessLabel(row: ChainQaRow, chain: ChainView): string | null {
  const memberDelta =
    row.snapshot_member_count !== chain.current_member_count;
  const messageDelta =
    row.snapshot_total_messages !== chain.current_total_messages;
  if (!memberDelta && !messageDelta) return null;

  if (memberDelta) {
    // Member count change is the more visible signal; phrase it that way and
    // collapse the message-count change into the same sentence.
    return `answered when chain had ${row.snapshot_member_count} ${
      row.snapshot_member_count === 1 ? 'conversation' : 'conversations'
    } (now ${chain.current_member_count})`;
  }
  // member count same, message count moved
  return `answered with ${row.snapshot_total_messages} prior ${
    row.snapshot_total_messages === 1 ? 'message' : 'messages'
  } (now ${chain.current_total_messages})`;
}
