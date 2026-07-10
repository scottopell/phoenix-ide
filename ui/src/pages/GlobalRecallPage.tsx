import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import { Link } from 'react-router-dom';
import { api } from '../api';
import type { GlobalOpenWorkResponse, GlobalRecallMessage, GlobalRecallSession } from '../api';
import { ConversationMarkdownAnchor } from '../components/conversationMarkdown';
import './GlobalRecallPage.css';

const MARKDOWN_COMPONENTS = { a: ConversationMarkdownAnchor };

export function GlobalRecallPage() {
  const [openWork, setOpenWork] = useState<GlobalOpenWorkResponse | null>(null);
  const [openWorkLoadingMore, setOpenWorkLoadingMore] = useState(false);
  const [sessions, setSessions] = useState<GlobalRecallSession[]>([]);
  const [sessionsHaveMore, setSessionsHaveMore] = useState(false);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [messages, setMessages] = useState<GlobalRecallMessage[]>([]);
  const [olderMessageCursor, setOlderMessageCursor] = useState<number | null>(null);
  const [olderMessagesLoading, setOlderMessagesLoading] = useState(false);
  const [question, setQuestion] = useState('');
  const [loading, setLoading] = useState(true);
  const [asking, setAsking] = useState(false);
  const [sessionLoading, setSessionLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copiedReference, setCopiedReference] = useState<string | null>(null);
  const activeSessionIdRef = useRef<string | null>(null);
  const sessionSelectionGenerationRef = useRef(0);

  useEffect(() => {
    activeSessionIdRef.current = activeSessionId;
    sessionSelectionGenerationRef.current += 1;
    setOlderMessagesLoading(false);
  }, [activeSessionId]);

  const refreshOpenWork = useCallback(() => {
    setError(null);
    api.getGlobalOpenWork()
      .then((work) => {
        setOpenWork(work);
        setError(null);
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }, []);

  const refreshSessions = useCallback(async () => {
    try {
      const page = await api.listGlobalRecallSessions();
      setSessions(page.sessions);
      setSessionsHaveMore(page.has_more);
      setActiveSessionId((prev) => prev ?? page.sessions[0]?.id ?? null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    setLoading(true);
    Promise.all([api.getGlobalOpenWork(), api.listGlobalRecallSessions()])
      .then(([work, page]) => {
        setOpenWork(work);
        setSessions((prev) => (prev.length > 0 ? prev : page.sessions));
        setSessionsHaveMore(page.has_more);
        setActiveSessionId((prev) => prev ?? page.sessions[0]?.id ?? null);
        setError(null);
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    setMessages([]);
    setOlderMessageCursor(null);
    if (!activeSessionId) {
      setSessionLoading(false);
      return;
    }
    const requestedSessionId = activeSessionId;
    let cancelled = false;
    setSessionLoading(true);
    api.getGlobalRecallSession(requestedSessionId)
      .then((res) => {
        if (!cancelled && requestedSessionId === activeSessionId) {
          setMessages(res.messages);
          setOlderMessageCursor(res.older_cursor);
          setError(null);
          setSessionLoading(false);
        }
      })
      .catch((e) => {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
          setSessionLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [activeSessionId]);

  const itemCount = useMemo(
    () => openWork?.groups.reduce((sum, group) => sum + group.items.length, 0) ?? 0,
    [openWork],
  );

  const loadMoreOpenWork = async () => {
    if (!openWork || !openWork.has_more || openWorkLoadingMore) return;
    setOpenWorkLoadingMore(true);
    setError(null);
    try {
      const page = await api.getGlobalOpenWork(itemCount);
      setOpenWork((prev) => prev ? {
        generated_at: page.generated_at,
        has_more: page.has_more,
        groups: mergeOpenWorkGroups(prev.groups, page.groups),
      } : page);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setOpenWorkLoadingMore(false);
    }
  };

  const loadMoreSessions = async () => {
    setError(null);
    try {
      const page = await api.listGlobalRecallSessions(sessions.length);
      setSessions((prev) => [...prev, ...page.sessions.filter((row) => !prev.some((p) => p.id === row.id))]);
      setSessionsHaveMore(page.has_more);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const loadOlderMessages = async () => {
    if (!activeSessionId || olderMessageCursor === null || olderMessagesLoading) return;
    const requestedSessionId = activeSessionId;
    const requestedCursor = olderMessageCursor;
    const requestedGeneration = sessionSelectionGenerationRef.current;
    setOlderMessagesLoading(true);
    setError(null);
    try {
      const page = await api.getGlobalRecallSession(requestedSessionId, requestedCursor);
      if (
        activeSessionIdRef.current === requestedSessionId
        && sessionSelectionGenerationRef.current === requestedGeneration
      ) {
        setMessages((prev) => [...page.messages, ...prev]);
        setOlderMessageCursor(page.older_cursor);
      }
    } catch (e) {
      if (
        activeSessionIdRef.current === requestedSessionId
        && sessionSelectionGenerationRef.current === requestedGeneration
      ) {
        setError(e instanceof Error ? e.message : String(e));
      }
    } finally {
      if (
        activeSessionIdRef.current === requestedSessionId
        && sessionSelectionGenerationRef.current === requestedGeneration
      ) {
        setOlderMessagesLoading(false);
      }
    }
  };

  const createSession = async () => {
    setError(null);
    try {
      const session = await api.createGlobalRecallSession(`Global Recall ${new Date().toLocaleString()}`);
      await refreshSessions();
      setActiveSessionId(session.id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const copyReference = async (reference: string) => {
    setError(null);
    setCopiedReference(null);
    try {
      await navigator.clipboard.writeText(reference);
      setCopiedReference(reference);
    } catch (e) {
      setError(e instanceof Error ? `Failed to copy reference: ${e.message}` : 'Failed to copy reference');
    }
  };

  const ask = async () => {
    if (!activeSessionId || !question.trim() || asking || sessionLoading) return;
    const text = question.trim();
    const submittedSessionId = activeSessionId;
    const submittedGeneration = sessionSelectionGenerationRef.current;
    const selectionIsCurrent = () => (
      activeSessionIdRef.current === submittedSessionId
      && sessionSelectionGenerationRef.current === submittedGeneration
    );
    setQuestion('');
    setAsking(true);
    setError(null);
    try {
      const res = await api.askGlobalRecallSession(submittedSessionId, text);
      if (selectionIsCurrent()) {
        setMessages((prev) => [...prev, res.user_message, res.assistant_message]);
      }
      void refreshSessions();
    } catch (e) {
      if (selectionIsCurrent()) {
        setError(e instanceof Error ? e.message : String(e));
        setQuestion(text);
      }
    } finally {
      setAsking(false);
    }
  };

  return (
    <div className="global-recall-page">
      <header className="global-recall-header">
        <div>
          <h1>Global Recall</h1>
          <p>Deterministic open work plus saved read-only recall sessions.</p>
        </div>
        <div className="global-recall-actions">
          <button type="button" onClick={refreshOpenWork}>Refresh work</button>
          <button type="button" onClick={createSession}>New recall session</button>
        </div>
      </header>

      {error && <div className="global-recall-error">{error}</div>}
      <div className="global-recall-sr-only" aria-live="polite">
        {copiedReference ? `Copied ${copiedReference}` : ''}
      </div>
      {loading ? <div className="global-recall-muted">Loading…</div> : null}

      <section className="global-recall-open-work">
        <div className="global-recall-section-title">
          <h2>Open Work</h2>
          <span>{itemCount} item{itemCount === 1 ? '' : 's'}</span>
        </div>
        {openWork?.groups.length === 0 ? <p className="global-recall-muted">No active work found.</p> : null}
        {openWork?.groups.map((group) => (
          <div className="global-recall-project" key={group.project_id ?? 'none'}>
            <h3>{group.project_name}</h3>
            {group.canonical_path && <div className="global-recall-path">{group.canonical_path}</div>}
            <div className="global-recall-items">
              {group.items.map((item) => (
                <article className="global-recall-item" key={item.reference}>
                  <div className="global-recall-item-main">
                    <Link to={item.href}>{item.title}</Link>
                    <div className="global-recall-meta">
                      <span>{item.source}</span>
                      <span>{item.mode}</span>
                      <span>{item.state}</span>
                      {item.member_count > 1 && <span>{item.member_count} convs</span>}
                      <span>{new Date(item.updated_at).toLocaleString()}</span>
                    </div>
                    <div className="global-recall-signals">
                      {item.signals.map((signal) => <span key={signal}>{signal}</span>)}
                    </div>
                    <div className="global-recall-work-meta">
                      <span>CURRENT {shortId(item.current_conversation_id)}</span>
                      {item.source === 'chain' && <span>ROOT {shortId(item.root_conversation_id)}</span>}
                      {item.worktree_path && <span>WORKTREE {item.worktree_path}</span>}
                      {item.task_title && <span>TASK {item.task_id ?? ''} {item.task_status ? `· ${item.task_status}` : ''} · {item.task_title}</span>}
                      {item.branch_name && <span>BRANCH {item.branch_name}{item.base_branch ? ` ← ${item.base_branch}` : ''}</span>}
                    </div>
                  </div>
                  <button type="button" onClick={() => { void copyReference(item.reference); }}>
                    Copy {item.reference}
                  </button>
                </article>
              ))}
            </div>
          </div>
        ))}
        {openWork?.has_more && (
          <button type="button" onClick={() => { void loadMoreOpenWork(); }} disabled={openWorkLoadingMore}>
            {openWorkLoadingMore ? 'Loading…' : 'Load more open work'}
          </button>
        )}
      </section>

      <section className="global-recall-session-pane">
        <aside className="global-recall-sessions">
          <h2>Recall Sessions</h2>
          {sessions.length === 0 && <p className="global-recall-muted">Create a session for cross-conversation analysis.</p>}
          {sessions.map((session) => (
            <button
              type="button"
              key={session.id}
              className={session.id === activeSessionId ? 'active' : ''}
              onClick={() => setActiveSessionId(session.id)}
              aria-pressed={session.id === activeSessionId}
            >
              <strong>{session.title}</strong>
              <span>{new Date(session.updated_at).toLocaleString()}</span>
            </button>
          ))}
          {sessionsHaveMore && (
            <button type="button" onClick={() => { void loadMoreSessions(); }}>
              Load more sessions
            </button>
          )}
        </aside>
        <div className="global-recall-chat">
          <div className="global-recall-messages" aria-live="polite">
            {olderMessageCursor !== null && !sessionLoading && (
              <button type="button" onClick={() => { void loadOlderMessages(); }} disabled={olderMessagesLoading}>
                {olderMessagesLoading ? 'Loading…' : 'Load older messages'}
              </button>
            )}
            {activeSessionId === null ? (
              <p className="global-recall-muted">No recall session selected.</p>
            ) : sessionLoading ? (
              <p className="global-recall-muted">Loading recall session…</p>
            ) : messages.length === 0 ? (
              <p className="global-recall-muted">Ask about strategy, handoffs, or history. Answers can search/read source conversations and cite links.</p>
            ) : messages.map((message) => (
              <div className={`global-recall-message ${message.role}`} key={message.id}>
                <div className="global-recall-message-role">{message.role}</div>
                {message.role === 'assistant' ? (
                  <ReactMarkdown components={MARKDOWN_COMPONENTS}>{message.content}</ReactMarkdown>
                ) : (
                  <p>{message.content}</p>
                )}
              </div>
            ))}
          </div>
          <form className="global-recall-composer" onSubmit={(e) => { e.preventDefault(); void ask(); }}>
            <label className="global-recall-sr-only" htmlFor="global-recall-question">Global Recall question</label>
            <textarea
              id="global-recall-question"
              value={question}
              onChange={(e) => setQuestion(e.target.value)}
              placeholder="Ask Global Recall to synthesize Phoenix history…"
              disabled={!activeSessionId || asking || sessionLoading}
            />
            <button type="submit" disabled={!activeSessionId || !question.trim() || asking || sessionLoading}>
              {asking ? 'Thinking…' : 'Ask'}
            </button>
          </form>
        </div>
      </section>
    </div>
  );
}

function shortId(id: string): string {
  return id.slice(0, 8);
}

function mergeOpenWorkGroups(
  current: GlobalOpenWorkResponse['groups'],
  incoming: GlobalOpenWorkResponse['groups'],
): GlobalOpenWorkResponse['groups'] {
  const groups = current.map((group) => ({ ...group, items: [...group.items] }));
  for (const incomingGroup of incoming) {
    const existing = groups.find((group) => group.project_id === incomingGroup.project_id);
    if (existing) {
      existing.items.push(...incomingGroup.items.filter((item) => !existing.items.some((currentItem) => currentItem.reference === item.reference)));
    } else {
      groups.push(incomingGroup);
    }
  }
  return groups;
}
