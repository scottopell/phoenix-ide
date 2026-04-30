// Chain page (REQ-CHN-003 / 004 / 005 / 006 / 007).
//
// Two-column layout: members on the left, Q&A panel (history + bottom-anchored
// input) on the right. SSE subscription is opened on mount and closed on
// unmount; tokens are demuxed by `chain_qa_id` so a sibling tab's question on
// the same chain does not bleed into this tab's render.
//
// Optimistic-update pattern: the user submits a question, we eagerly render an
// in-flight ChainQaRow with `answer = ''`, and SSE token events append to its
// answer in component state. On `chain_qa_completed` / `chain_qa_failed` we
// re-fetch the chain to pick up the canonical persisted row (which also bumps
// `current_member_count` / `current_total_messages` if the chain advanced
// while the question was streaming).
//
// REQ-CHN-006 visual independence: each Q&A entry renders as a self-contained
// card with no chat-style ligatures (no thread/reply lines, no avatar
// continuity, no indenting follow-ups). The `ChainQaCard` keeps that promise
// structurally — siblings are flat list items separated by a vertical gap.

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
  const [draft, setDraft] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [sseLost, setSseLost] = useState(false);

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
        setDraft('');
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

  const handleReask = (question: string) => {
    setDraft(question);
    void submit(question);
  };

  // Sort persisted rows by created_at ascending; in-flight entries are appended
  // after them in submission order. The "most recent immediately above the
  // input" rule (REQ-CHN-005) falls out of this ordering plus the bottom-
  // anchored layout.
  const renderableQas = useMemo(() => {
    const persisted: ChainQaRow[] = chain?.qa_history.slice() ?? [];
    persisted.sort((a, b) => (a.created_at < b.created_at ? -1 : 1));
    const inflightList = Object.values(inflight);
    return { persisted, inflightList };
  }, [chain, inflight]);

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
          onRetryConnection={() => {
            // Force the SSE effect to re-run by re-fetching first; the
            // EventSource itself will already be auto-reconnecting under the
            // hood, but this gives the user a clear "I tried" affordance.
            setSseLost(false);
            if (rootConvId) void refresh(rootConvId);
          }}
        />
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Header (REQ-CHN-007 inline name edit)
// ---------------------------------------------------------------------------

interface ChainPageHeaderProps {
  chain: ChainView;
  onRename: (name: string | null) => Promise<void>;
}

function ChainPageHeader({ chain, onRename }: ChainPageHeaderProps) {
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
// Q&A column
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
  onRetryConnection,
}: ChainQaColumnProps) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  // Autoscroll on new content so the most-recent Q&A or live token sits near
  // the input. We don't fight the user if they've scrolled up — only stick to
  // the bottom when we're already there.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const nearBottom =
      el.scrollHeight - el.scrollTop - el.clientHeight < 200;
    if (nearBottom) {
      el.scrollTop = el.scrollHeight;
    }
  }, [persisted, inflight]);

  const isEmpty = persisted.length === 0 && inflight.length === 0;

  return (
    <section className="chain-qa" aria-label="Chain questions and answers">
      <div className="chain-qa-scroll" ref={scrollRef}>
        {isEmpty ? (
          <div className="chain-qa-empty">
            <p>No questions yet.</p>
            <p className="chain-qa-empty-hint">
              Ask the chain anything — answers see the full content of every
              conversation in this chain.
            </p>
          </div>
        ) : (
          <ul className="chain-qa-list">
            {persisted.map((row) => (
              <li key={row.id}>
                <ChainQaCard
                  row={row}
                  chain={chain}
                  onReask={onReask}
                />
              </li>
            ))}
            {inflight.map((entry) => (
              <li key={entry.chainQaId}>
                <ChainQaInflightCard entry={entry} onReask={onReask} />
              </li>
            ))}
          </ul>
        )}
      </div>
      {sseLost && (
        <div className="chain-qa-sse-lost" role="status">
          <span>Connection lost — tokens may be delayed.</span>
          <button type="button" className="btn-link" onClick={onRetryConnection}>
            Retry
          </button>
        </div>
      )}
      <form className="chain-qa-form" onSubmit={onSubmit}>
        <textarea
          className="chain-qa-input"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              if (draft.trim().length > 0 && !submitting) {
                // Pretend the form submitted — keeps the single submit path.
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
        <button
          type="submit"
          className="btn-primary chain-qa-submit"
          disabled={submitting || draft.trim().length === 0}
        >
          {submitting ? 'Sending…' : 'Ask'}
        </button>
      </form>
    </section>
  );
}

// ---------------------------------------------------------------------------
// Q&A cards (per-status rendering, REQ-CHN-005)
// ---------------------------------------------------------------------------

interface ChainQaCardProps {
  row: ChainQaRow;
  chain: ChainView;
  onReask: (question: string) => void;
}

function ChainQaCard({ row, chain, onReask }: ChainQaCardProps) {
  const stale = stalenessLabel(row, chain);
  return (
    <article className={`chain-qa-card chain-qa-card--${row.status}`}>
      <div className="chain-qa-question">
        <span className="chain-qa-question-prefix">Q.</span>
        <span className="chain-qa-question-text">{row.question}</span>
      </div>
      {renderAnswerByStatus(row, onReask)}
      <div className="chain-qa-meta">
        <time dateTime={row.created_at}>{formatShortDateTime(row.created_at)}</time>
        {stale && <span className="chain-qa-stale">{stale}</span>}
      </div>
    </article>
  );
}

function renderAnswerByStatus(
  row: ChainQaRow,
  onReask: (q: string) => void,
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
    // Our own in-flight Q&As are rendered via ChainQaInflightCard, not this
    // function — so reaching here means another subscriber is currently
    // generating this row.
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
            onClick={() => onReask(row.question)}
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
          onClick={() => onReask(row.question)}
        >
          Re-ask
        </button>
      </div>
    </div>
  );
}

interface ChainQaInflightCardProps {
  entry: InflightQa;
  onReask: (q: string) => void;
}

function ChainQaInflightCard({ entry, onReask }: ChainQaInflightCardProps) {
  return (
    <article className="chain-qa-card chain-qa-card--in_flight">
      <div className="chain-qa-question">
        <span className="chain-qa-question-prefix">Q.</span>
        <span className="chain-qa-question-text">{entry.question}</span>
      </div>
      {entry.error ? (
        <div className="chain-qa-answer chain-qa-answer--failed">
          {entry.answer.length > 0 && (
            <div className="chain-qa-partial chain-qa-markdown">
              <ReactMarkdown remarkPlugins={REMARK_PLUGINS}>
                {entry.answer}
              </ReactMarkdown>
            </div>
          )}
          <div className="chain-qa-failure">
            <span className="chain-qa-failure-label">Failed: {entry.error}</span>
            <button
              type="button"
              className="btn-link"
              onClick={() => onReask(entry.question)}
            >
              Re-ask
            </button>
          </div>
        </div>
      ) : entry.preToken ? (
        <div className="chain-qa-answer chain-qa-answer--skeleton" aria-live="polite">
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
            {entry.answer}
          </ReactMarkdown>
          <span className="chain-qa-cursor" aria-hidden="true">▍</span>
        </div>
      )}
    </article>
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
