import { memo, useEffect, type FormEvent } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { ChainView, ChainQaRow } from '../api';
import type { InflightQa } from '../chain';
import { formatShortDateTime } from '../utils';

const REMARK_PLUGINS = [remarkGfm];

interface ChainQaColumnProps {
  chain: ChainView;
  persisted: ChainQaRow[];
  inflight: InflightQa[];
  draft: string;
  setDraft: (s: string) => void;
  submitting: boolean;
  sseLost: boolean;
  onSubmit?: (e: FormEvent<HTMLFormElement>) => void;
  onReask: (question: string) => void;
  activeTextareaRef: React.RefObject<HTMLTextAreaElement>;
  onRetryConnection: () => void;
  disabled?: boolean;
  /** ProductConversation owns whether opening Recall should move focus. */
  autoFocusActive?: boolean;
  onActiveTextareaFocused?: () => void;
}

export function ChainQaColumn({
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
  disabled = false,
  autoFocusActive = true,
  onActiveTextareaFocused,
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
              disabled={disabled}
              {...(onSubmit ? { onSubmit } : {})}
              activeTextareaRef={activeTextareaRef}
              autoFocusActive={autoFocusActive}
              {...(onActiveTextareaFocused ? { onActiveTextareaFocused } : {})}
            />
          </li>
          {[...inflight].reverse().map((entry) => (
            <li key={entry.chainQaId}>
              <ChainQaPairCard
                variant={entry.error ? 'inflight-failed' : 'inflight-streaming'}
                inflightEntry={entry}
                {...(!disabled ? { onReask } : {})}
              />
            </li>
          ))}
          {[...persisted].reverse().map((row) => (
            <li key={row.id}>
              <ChainQaPairCard
                variant="persisted"
                row={row}
                chain={chain}
                {...(!disabled ? { onReask } : {})}
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
  disabled?: boolean;
  onSubmit?: (e: FormEvent<HTMLFormElement>) => void;
  activeTextareaRef?: React.RefObject<HTMLTextAreaElement>;
  autoFocusActive?: boolean;
  onActiveTextareaFocused?: () => void;
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
  disabled = false,
  onSubmit,
  activeTextareaRef,
  autoFocusActive = true,
  onActiveTextareaFocused,
}: ChainQaPairCardProps) {
  // The product conversation owns opening focus; ChainPage retains the
  // default. This effect runs after a lazy ChainQaColumn has mounted its ref.
  useEffect(() => {
    if (!autoFocusActive || disabled || !activeTextareaRef?.current) return;
    activeTextareaRef.current.focus();
    onActiveTextareaFocused?.();
  }, [activeTextareaRef, autoFocusActive, disabled, onActiveTextareaFocused]);

  const canSubmit = draft.trim().length > 0 && !submitting && !disabled && !!onSubmit;

  return (
    <article className="chain-qa-pair chain-qa-pair--active">
      <form className="chain-qa-pair-form" {...(onSubmit ? { onSubmit } : {})}>
        <div className="chain-qa-pair-row">
          <span className="chain-qa-pair-label">Q:</span>
          <div className="chain-qa-pair-content">
            <textarea
              ref={activeTextareaRef}
              className="chain-qa-input"
              value={draft}
              disabled={disabled}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault();
                  if (canSubmit) {
                    // Single submit path — mirror the form submit.
                    onSubmit?.({
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

// Memoized so a token landing on one in-flight Q&A re-renders only that card.
// `TOKEN_APPENDED` replaces just the streaming entry's object identity; sibling
// in-flight entries keep theirs, so a shallow prop compare bails. Mirrors
// PersistedPairCard's memoization for the same per-token re-parse problem.
const InflightPairCard = memo(function InflightPairCard({ inflightEntry, onReask }: ChainQaPairCardProps) {
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
                  disabled={!onReask}
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
});

const PersistedPairCard = memo(function PersistedPairCard({ row, chain, onReask }: ChainQaPairCardProps) {
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
});

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
            disabled={!onReask}
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
          disabled={!onReask}
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
    row.chain_members_at_answer !== chain.current_member_count;
  const messageDelta =
    row.chain_messages_at_answer !== chain.current_total_messages;
  if (!memberDelta && !messageDelta) return null;

  if (memberDelta) {
    // Member count change is the more visible signal; phrase it that way and
    // collapse the message-count change into the same sentence.
    return `answered when chain had ${row.chain_members_at_answer} ${
      row.chain_members_at_answer === 1 ? 'conversation' : 'conversations'
    } (now ${chain.current_member_count})`;
  }
  // member count same, message count moved
  return `answered with ${row.chain_messages_at_answer} prior ${
    row.chain_messages_at_answer === 1 ? 'message' : 'messages'
  } (now ${chain.current_total_messages})`;
}
