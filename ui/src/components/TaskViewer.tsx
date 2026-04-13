import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api';
import type { TaskEntry, Conversation } from '../api';
import './TaskViewer.css';

interface TaskViewerProps {
  task: TaskEntry;
  tasksDir: string;
  /** The conversation the user is currently viewing — used as the seed parent
   *  when starting a "work on this task" sub-conversation. May be null if the
   *  task viewer is shown outside a conversation context. */
  parentConversation: Conversation | null;
  onBack: () => void;
}

const STATUS_CLASS: Record<string, string> = {
  'in-progress': 'task-viewer-status-in-progress',
  'ready': 'task-viewer-status-ready',
  'blocked': 'task-viewer-status-blocked',
  'brainstorming': 'task-viewer-status-brainstorming',
  'done': 'task-viewer-status-done',
  'wont-do': 'task-viewer-status-wont-do',
};

const TERMINAL_STATUSES = new Set(['done', 'wont-do']);

export function TaskViewer({ task, tasksDir, parentConversation, onBack }: TaskViewerProps) {
  const navigate = useNavigate();
  const [content, setContent] = useState<string | null>(null);
  const [rawContent, setRawContent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [seeding, setSeeding] = useState(false);
  const [seedError, setSeedError] = useState<string | null>(null);

  // Prefer the path the backend already resolved (REQ-TASK-PANEL-START); fall
  // back to reconstructing it from id+priority+status+slug for older API
  // responses that may not include the field.
  const filename = `${task.id}-${task.priority}-${task.status}--${task.slug}.md`;
  const filePath = task.path || `${tasksDir}/${filename}`;

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    setContent(null);

    fetch(`/api/files/read?path=${encodeURIComponent(filePath)}`)
      .then(async (resp) => {
        if (!resp.ok) {
          const err = await resp.json().catch(() => ({ error: 'Unknown error' }));
          throw new Error(err.error || 'Failed to read file');
        }
        return resp.json();
      })
      .then((data) => {
        if (!cancelled) {
          setRawContent(data.content);
          setContent(stripFrontmatter(data.content));
          setLoading(false);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err.message);
          setLoading(false);
        }
      });

    return () => { cancelled = true; };
  }, [filePath]);

  const isTerminal = TERMINAL_STATUSES.has(task.status);
  // The button needs the task body to build the prompt. We can show it as soon
  // as content is loaded; until then it's disabled with a loading hint.
  const canStartWork = !isTerminal && parentConversation !== null;

  const handleStartWork = async () => {
    if (!parentConversation || rawContent === null) return;
    setSeeding(true);
    setSeedError(null);
    try {
      const promptText = buildTaskPrompt(task, rawContent);
      const seedLabel = `Work on task ${task.id}: ${task.slug}`;
      const messageId =
        crypto.randomUUID?.() ??
        `seed-${Date.now()}-${Math.random().toString(36).slice(2)}`;
      const newConv = await api.createConversation(
        parentConversation.cwd,
        '', // empty — server accepts empty text when seed_parent_id is set
        messageId,
        parentConversation.model ?? undefined,
        [],
        'auto',
        null,
        parentConversation.id,
        seedLabel,
      );
      try {
        localStorage.setItem(`seed-draft:${newConv.id}`, promptText);
      } catch {
        // ignore — non-fatal
      }
      if (newConv.slug) {
        navigate(`/c/${newConv.slug}`);
      }
    } catch (err) {
      setSeedError(err instanceof Error ? err.message : 'Failed to start task');
      setSeeding(false);
    }
  };

  return (
    <div className="task-viewer">
      <div className="task-viewer-header">
        <button className="task-viewer-back" onClick={onBack}>
          &larr; Back
        </button>
        <span className="task-viewer-name">{task.id}</span>
        {canStartWork && (
          <button
            className="task-viewer-start-work"
            onClick={handleStartWork}
            disabled={seeding || rawContent === null}
            title={
              rawContent === null
                ? 'Loading task content...'
                : `Start a new conversation pre-filled with this task`
            }
          >
            {seeding ? 'Starting...' : 'Start working'}
          </button>
        )}
      </div>
      {seedError && (
        <div className="task-viewer-seed-error">{seedError}</div>
      )}

      <div className="task-viewer-body">
        {/* Details section */}
        <div className="task-viewer-section">
          <div className="task-viewer-section-title">Details</div>
          <div className="task-viewer-detail-row">
            <span className="task-viewer-detail-label">Status</span>
            <span className={`task-viewer-detail-value ${STATUS_CLASS[task.status] || ''}`}>
              {task.status}
            </span>
          </div>
          <div className="task-viewer-detail-row">
            <span className="task-viewer-detail-label">Priority</span>
            <span className="task-viewer-detail-value">{task.priority}</span>
          </div>
          <div className="task-viewer-detail-row">
            <span className="task-viewer-detail-label">Slug</span>
            <span className="task-viewer-detail-value">{task.slug}</span>
          </div>
          <div className="task-viewer-detail-row">
            <span className="task-viewer-detail-label">File</span>
            <span className="task-viewer-detail-value">{filename}</span>
          </div>
          {task.conversation_slug && (
            <div className="task-viewer-detail-row">
              <span className="task-viewer-detail-label">Conversation</span>
              <button
                className="task-viewer-conv-link"
                onClick={() => navigate(`/c/${task.conversation_slug}`)}
              >
                Go to conversation &rarr;
              </button>
            </div>
          )}
        </div>

        {/* Content section */}
        <div className="task-viewer-section">
          <div className="task-viewer-section-title">Content</div>
          {loading && (
            <div className="task-viewer-content task-viewer-content-loading">Loading...</div>
          )}
          {error && (
            <div className="task-viewer-content task-viewer-content-error">{error}</div>
          )}
          {content !== null && !loading && (
            <div className="task-viewer-content">{content}</div>
          )}
        </div>
      </div>
    </div>
  );
}

/** Strip YAML frontmatter (--- delimited block at the top) from file content */
function stripFrontmatter(content: string): string {
  const lines = content.split('\n');
  if (lines[0]?.trim() !== '---') return content;
  const endIdx = lines.indexOf('---', 1);
  if (endIdx === -1) return content;
  return lines.slice(endIdx + 1).join('\n').trim();
}

/** Build the seed-draft prompt the user will see in the new conversation's
 *  input area. The body is the full task markdown including frontmatter — we
 *  hand the LLM the same view a developer would see when opening the file. */
function buildTaskPrompt(task: TaskEntry, body: string): string {
  return `I want to work on task ${task.id}.

Here's the task:

---
${body.trim()}
---

Please read the scope carefully and start executing. Ask before doing anything destructive (commits, force pushes, deleting files outside the task's stated scope). If the scope is unclear in any way, stop and ask for clarification before writing code.`;
}
