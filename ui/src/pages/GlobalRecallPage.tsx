import { useCallback, useEffect, useMemo, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import { Link } from 'react-router-dom';
import { api } from '../api';
import type { GlobalOpenWorkResponse, GlobalRecallMessage, GlobalRecallSession } from '../api';
import { ConversationMarkdownAnchor } from '../components/conversationMarkdown';
import './GlobalRecallPage.css';

const MARKDOWN_COMPONENTS = { a: ConversationMarkdownAnchor };

export function GlobalRecallPage() {
  const [openWork, setOpenWork] = useState<GlobalOpenWorkResponse | null>(null);
  const [sessions, setSessions] = useState<GlobalRecallSession[]>([]);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [messages, setMessages] = useState<GlobalRecallMessage[]>([]);
  const [question, setQuestion] = useState('');
  const [loading, setLoading] = useState(true);
  const [asking, setAsking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refreshOpenWork = useCallback(() => {
    api.getGlobalOpenWork()
      .then(setOpenWork)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }, []);

  const refreshSessions = useCallback(() => {
    api.listGlobalRecallSessions()
      .then((rows) => {
        setSessions(rows);
        setActiveSessionId((prev) => prev ?? rows[0]?.id ?? null);
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }, []);

  useEffect(() => {
    setLoading(true);
    Promise.all([api.getGlobalOpenWork(), api.listGlobalRecallSessions()])
      .then(([work, rows]) => {
        setOpenWork(work);
        setSessions(rows);
        setActiveSessionId(rows[0]?.id ?? null);
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    if (!activeSessionId) {
      setMessages([]);
      return;
    }
    api.getGlobalRecallSession(activeSessionId)
      .then((res) => setMessages(res.messages))
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }, [activeSessionId]);

  const itemCount = useMemo(
    () => openWork?.groups.reduce((sum, group) => sum + group.items.length, 0) ?? 0,
    [openWork],
  );

  const createSession = async () => {
    setError(null);
    const session = await api.createGlobalRecallSession(`Global Recall ${new Date().toLocaleString()}`);
    refreshSessions();
    setActiveSessionId(session.id);
  };

  const ask = async () => {
    if (!activeSessionId || !question.trim() || asking) return;
    const text = question.trim();
    setQuestion('');
    setAsking(true);
    setError(null);
    try {
      const submittedSessionId = activeSessionId;
      const res = await api.askGlobalRecallSession(submittedSessionId, text);
      setActiveSessionId((current) => {
        if (current === submittedSessionId) {
          setMessages((prev) => [...prev, res.user_message, res.assistant_message]);
        }
        return current;
      });
      refreshSessions();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setQuestion(text);
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
                  <button type="button" onClick={() => navigator.clipboard.writeText(item.reference)}>
                    Copy {item.reference}
                  </button>
                </article>
              ))}
            </div>
          </div>
        ))}
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
            >
              <strong>{session.title}</strong>
              <span>{new Date(session.updated_at).toLocaleString()}</span>
            </button>
          ))}
        </aside>
        <div className="global-recall-chat">
          <div className="global-recall-messages">
            {activeSessionId === null ? (
              <p className="global-recall-muted">No recall session selected.</p>
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
            <textarea
              value={question}
              onChange={(e) => setQuestion(e.target.value)}
              placeholder="Ask Global Recall to synthesize Phoenix history…"
              disabled={!activeSessionId || asking}
            />
            <button type="submit" disabled={!activeSessionId || !question.trim() || asking}>
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
