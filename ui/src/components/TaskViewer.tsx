import { useState, useEffect } from 'react';
import type { TaskEntry } from '../api';
import './TaskViewer.css';

interface TaskViewerProps {
  task: TaskEntry;
  tasksDir: string;
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

export function TaskViewer({ task, tasksDir, onBack }: TaskViewerProps) {
  const [content, setContent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const filename = `${task.id}-${task.priority}-${task.status}--${task.slug}.md`;
  const filePath = `${tasksDir}/${filename}`;

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

  return (
    <div className="task-viewer">
      <div className="task-viewer-header">
        <button className="task-viewer-back" onClick={onBack}>
          &larr; Back
        </button>
        <span className="task-viewer-name">{task.id}</span>
      </div>

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
