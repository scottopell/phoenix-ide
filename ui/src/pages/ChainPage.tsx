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
// Optimistic-update pattern: the user submits a question, we eagerly render an
// in-flight pair card with `answer = ''`, and SSE token events append to its
// answer in component state. On `chain_qa_completed` / `chain_qa_failed` we
// re-fetch the chain to pick up the canonical persisted row (which also bumps
// `current_member_count` / `current_total_messages` if the chain advanced
// while the question was streaming).
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

// Markdown plugin set, hoisted so the array identity is stable across
// renders (matches the pattern in StreamingMessage.tsx).
const REMARK_PLUGINS = [remarkGfm];
import { formatShortDateTime } from '../utils';

/** Live, not-yet-persisted Q&A entry. We keep these in component state and
 *  merge them into the rendered list as if they were rows. On stream completion
 *  the entry is dropped and the persisted row from the refetched ChainView
 *  takes its place. */
interface InflightQa {
  chainQaId: string;
  question: string;
  answer: string;
  /** True until the first token arrives — drives the skeleton vs streaming
   *  visual split (REQ-CHN-004 pre-token vs streaming). */
  preToken: boolean;
  /** When the SSE stream errors, we surface the error inline and offer
   *  re-ask while still keeping the question visible. */
  error: string | null;
}

export function ChainPage() {
  const { rootConvId } = useParams<{ rootConvId: string }>();
  const navigate = useNavigate();

  const [chain, setChain] = useState<ChainView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [inflight, setInflight] = useState<Record<string, InflightQa>>({});
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  // Submission order for in-flight entries. We display newest-first, so we
  // walk this list in reverse on render. (Object key order on `inflight`
  // alone is not a reliable ordering source — Record iteration order is not
  // a guarantee we want to rely on for UI ordering.)
  const [inflightOrder, setInflightOrder] = useState<string[]>([]);
  const [draft, setDraft] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [sseLost, setSseLost] = useState(false);

  // Imperative handle to the active pair's textarea so we can refocus it
  // immediately after submit (the user agreed they should be able to type the
  // next question without waiting for the answer).
  const activeTextareaRef = useRef<HTMLTextAreaElement | null>(null);

  // Keep a ref so SSE callbacks can append tokens without re-subscribing.
  const inflightRef = useRef(inflight);
  inflightRef.current = inflight;

  /** Refresh the chain snapshot. Used after submit/complete/fail and on the
   *  first mount. Network failure does not blow the page away if we already
   *  have a snapshot — REQ-CHN-005 says history should persist. */
  const refresh = useCallback(async (rootId: string) => {
    try {
      const view = await api.getChain(rootId);
      setChain(view);
      setLoadError(null);
      return view;
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to load chain';
      setLoadError(msg);
      return null;
    }
  }, []);

  // Initial load.
  useEffect(() => {
    if (!rootConvId) return;
    let cancelled = false;
    setLoading(true);
    api
      .getChain(rootConvId)
      .then((view) => {
        if (cancelled) return;
        setChain(view);
        setLoadError(null);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setLoadError(err instanceof Error ? err.message : 'Failed to load chain');
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [rootConvId]);

  // SSE subscription. We open one EventSource for the chain and demux events
  // by chain_qa_id against our own in-flight buffer. Close on unmount or root
  // change so leaving the page tears down the connection cleanly.
  useEffect(() => {
    if (!rootConvId) return;

    const handleEvent = (evt: ChainSseEventData) => {
      // We only render tokens for in-flight Q&As we know about (i.e., ours).
      // Sibling-tab Q&A on the same chain falls through to a refetch when it
      // completes — but its tokens never bleed into our buffer.
      if (evt.type === 'chain_qa_token') {
        setInflight((prev) => {
          const cur = prev[evt.chain_qa_id];
          if (!cur) return prev;
          return {
            ...prev,
            [evt.chain_qa_id]: {
              ...cur,
              answer: cur.answer + evt.delta,
              preToken: false,
            },
          };
        });
      } else if (evt.type === 'chain_qa_completed') {
        // Drop the in-flight entry and refetch to pick up the persisted row.
        setInflight((prev) => {
          if (!(evt.chain_qa_id in prev)) return prev;
          const next = { ...prev };
          delete next[evt.chain_qa_id];
          return next;
        });
        setInflightOrder((prev) => prev.filter((id) => id !== evt.chain_qa_id));
        // Refetch so the persisted ChainQaRow replaces the optimistic entry.
        // Even for sibling-tab questions, refetching is the simplest way to
        // surface their newly-persisted answer.
        void refresh(rootConvId);
      } else if (evt.type === 'chain_qa_failed') {
        setInflight((prev) => {
          const cur = prev[evt.chain_qa_id];
          if (!cur) return prev;
          return {
            ...prev,
            [evt.chain_qa_id]: {
              ...cur,
              answer: evt.partial_answer ?? cur.answer,
              error: evt.error,
              preToken: false,
            },
          };
        });
        // Refetch: server has persisted the failed row with `status=failed`,
        // so on next render we let the persisted row drive UI and drop our
        // optimistic entry.
        void refresh(rootConvId).then(() => {
          setInflight((prev) => {
            if (!(evt.chain_qa_id in prev)) return prev;
            const next = { ...prev };
            delete next[evt.chain_qa_id];
            return next;
          });
          setInflightOrder((prev) =>
            prev.filter((id) => id !== evt.chain_qa_id),
          );
        });
      }
    };

    const handleErr = () => {
      // EventSource will auto-reconnect on its own. We surface a "connection
      // lost" affordance only if there is in-flight work that depends on it.
      if (Object.keys(inflightRef.current).length > 0) {
        setSseLost(true);
      }
    };

    const es = subscribeToChainStream(rootConvId, handleEvent, handleErr);
    setSseLost(false);
    return () => es.close();
  }, [rootConvId, refresh]);

  const submit = useCallback(
    async (question: string) => {
      if (!rootConvId || !chain) return;
      const trimmed = question.trim();
      if (!trimmed) return;
      setSubmitting(true);
      try {
        const { chain_qa_id } = await api.submitChainQuestion(rootConvId, trimmed);
        setInflight((prev) => ({
          ...prev,
          [chain_qa_id]: {
            chainQaId: chain_qa_id,
            question: trimmed,
            answer: '',
            preToken: true,
            error: null,
          },
        }));
        setInflightOrder((prev) => [...prev, chain_qa_id]);
        setDraft('');
        // Refocus the active textarea so the user can immediately type the
        // next question. We're reusing the same DOM node; it just got cleared.
        // Defer to a microtask so React has a chance to flush the value reset
        // before we steal focus back.
        queueMicrotask(() => {
          activeTextareaRef.current?.focus();
        });
      } catch (err) {
        // Surface the error to the user via a transient banner but keep the
        // draft so they can retry without retyping.
        const msg = err instanceof Error ? err.message : 'Failed to submit question';
        setLoadError(msg);
      } finally {
        setSubmitting(false);
      }
    },
    [rootConvId, chain],
  );

  const handleSubmit = (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    void submit(draft);
  };

  /** Re-ask: populate the active textarea with the original question and
   *  focus it. Do NOT auto-submit — REQ-CHN-007's editing pattern preserves
   *  user agency, and consistency with that precedent matters here. */
  const handleReask = (question: string) => {
    setDraft(question);
    queueMicrotask(() => {
      activeTextareaRef.current?.focus();
    });
  };

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
            setChain(updated);
          } catch (err) {
            setLoadError(
              err instanceof Error ? err.message : 'Failed to rename chain',
            );
          }
        }}
        onArchive={async () => {
          if (!rootConvId) return;
          try {
            await api.archiveChain(rootConvId);
            navigate('/');
          } catch (err) {
            setLoadError(
              err instanceof Error ? err.message : 'Failed to archive chain',
            );
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
            setSseLost(false);
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
            setLoadError(
              err instanceof Error ? err.message : 'Failed to delete chain',
            );
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
  onArchive: () => void | Promise<void>;
  onDelete: () => void;
}

function ChainPageHeader({ chain, onRename, onArchive, onDelete }: ChainPageHeaderProps) {
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
          onClick={() => void onArchive()}
        >
          Archive
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
              disabled={submitting}
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
