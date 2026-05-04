import { useState, useEffect } from 'react';
import type { SkillEntry } from '../api';
import './SkillViewer.css';

interface SkillViewerProps {
  skill: SkillEntry;
  onBack: () => void;
}

/** Strip YAML frontmatter (--- delimited block at the top) from file content */
function stripFrontmatter(content: string): string {
  const lines = content.split('\n');
  if (lines[0]?.trim() !== '---') return content;
  const endIdx = lines.indexOf('---', 1);
  if (endIdx === -1) return content;
  return lines.slice(endIdx + 1).join('\n').trim();
}

/** Extract the directory containing the SKILL.md */
function skillDir(path: string): string {
  const lastSlash = path.lastIndexOf('/');
  return lastSlash >= 0 ? path.substring(0, lastSlash + 1) : path;
}

/** Extract the project name or "User" from a skill's absolute path */
function projectLabel(skill: SkillEntry): string {
  if (skill.source === 'builtin') return 'Built-in';
  const path = skill.path;
  const markers = ['.claude/skills', '.agents/skills'];
  for (const marker of markers) {
    const idx = path.indexOf(marker);
    if (idx !== -1) {
      const prefix = path.substring(0, idx).replace(/\/$/, '');
      const lastSlash = prefix.lastIndexOf('/');
      const dirName = lastSlash >= 0 ? prefix.substring(lastSlash + 1) : prefix;
      if (!dirName || /^\/Users\/[^/]+$/.test(prefix) || /^\/home\/[^/]+$/.test(prefix)) {
        return 'User';
      }
      return dirName;
    }
  }
  return 'Unknown';
}

export function SkillViewer({ skill, onBack }: SkillViewerProps) {
  const [promptContent, setPromptContent] = useState<string | null>(null);
  const [promptError, setPromptError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setPromptError(null);
    setPromptContent(null);

    // Built-ins are extracted to disk at server startup, so they share the
    // same read endpoint as filesystem skills.
    fetch(`/api/files/read?path=${encodeURIComponent(skill.path)}`)
      .then(async (resp) => {
        if (!resp.ok) {
          const err = await resp.json().catch(() => ({ error: 'Unknown error' }));
          throw new Error(err.error || 'Failed to read file');
        }
        return resp.json();
      })
      .then((data) => {
        if (!cancelled) {
          setPromptContent(stripFrontmatter(data.content));
          setLoading(false);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setPromptError(err.message);
          setLoading(false);
        }
      });

    return () => { cancelled = true; };
  }, [skill.path]);

  const handleInsert = () => {
    window.dispatchEvent(
      new CustomEvent('phoenix:insert-draft', {
        detail: { text: `/${skill.name} ` },
      }),
    );
    onBack();
  };

  return (
    <div className="skill-viewer">
      <div className="skill-viewer-header">
        <button className="skill-viewer-back" onClick={onBack}>
          &larr; Back
        </button>
        <span className="skill-viewer-name">/{skill.name}</span>
      </div>

      <div className="skill-viewer-body">
        {/* Description section */}
        <div className="skill-viewer-section">
          <div className="skill-viewer-section-title">Description</div>
          <div className="skill-viewer-description">{skill.description}</div>
        </div>

        {/* Details section */}
        <div className="skill-viewer-section">
          <div className="skill-viewer-section-title">Details</div>
          <div className="skill-viewer-detail-row">
            <span className="skill-viewer-detail-label">Project</span>
            <span className="skill-viewer-detail-value">{projectLabel(skill)}</span>
          </div>
          <div className="skill-viewer-detail-row">
            <span className="skill-viewer-detail-label">Source</span>
            <span className="skill-viewer-detail-value">{skill.source}</span>
          </div>
          {skill.argument_hint && (
            <div className="skill-viewer-detail-row">
              <span className="skill-viewer-detail-label">Args</span>
              <span className="skill-viewer-detail-value">{skill.argument_hint}</span>
            </div>
          )}
          {skill.source !== 'builtin' && (
            <div className="skill-viewer-detail-row">
              <span className="skill-viewer-detail-label">Path</span>
              <span className="skill-viewer-detail-value">{skillDir(skill.path)}</span>
            </div>
          )}
        </div>

        {/* Prompt section */}
        <div className="skill-viewer-section">
          <div className="skill-viewer-section-title">Prompt</div>
          {loading && (
            <div className="skill-viewer-prompt skill-viewer-prompt-loading">Loading...</div>
          )}
          {promptError && (
            <div className="skill-viewer-prompt skill-viewer-prompt-error">{promptError}</div>
          )}
          {promptContent !== null && !loading && (
            <div className="skill-viewer-prompt">{promptContent}</div>
          )}
        </div>
      </div>

      <div className="skill-viewer-footer">
        <button className="skill-viewer-insert-btn" onClick={handleInsert}>
          Insert /{skill.name} into input
        </button>
      </div>
    </div>
  );
}
