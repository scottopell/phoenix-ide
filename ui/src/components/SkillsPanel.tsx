import { useState, useEffect } from 'react';
import { api } from '../api';
import type { SkillEntry } from '../api';
import './SkillsPanel.css';

interface SkillsPanelProps {
  conversationId: string | undefined;
}

export function SkillsPanel({ conversationId }: SkillsPanelProps) {
  const [skills, setSkills] = useState<SkillEntry[]>([]);
  const [expanded, setExpanded] = useState(false);

  useEffect(() => {
    if (!conversationId) {
      setSkills([]);
      return;
    }

    let cancelled = false;
    const controller = new AbortController();

    api.listConversationSkills(conversationId, controller.signal)
      .then(resp => {
        if (!cancelled) setSkills(resp.skills);
      })
      .catch(() => {
        if (!cancelled) setSkills([]);
      });

    return () => {
      cancelled = true;
      controller.abort();
    };
  }, [conversationId]);

  if (skills.length === 0 && !expanded) {
    return null;
  }

  const handleSkillClick = (skill: SkillEntry) => {
    const text = `/${skill.name} `;
    window.dispatchEvent(new CustomEvent('insert-skill', { detail: { text } }));
  };

  return (
    <div className="skills-panel">
      <button className="skills-panel-header" onClick={() => setExpanded(!expanded)}>
        <span className={`skills-panel-chevron ${expanded ? 'expanded' : ''}`}>&#9654;</span>
        <span className="skills-panel-summary">
          {skills.length === 0
            ? 'No skills'
            : <>Skills &middot; {skills.length} available</>
          }
        </span>
      </button>
      {expanded && (
        <div className="skills-panel-body">
          {skills.length === 0 ? (
            <div className="skills-empty">No skills discovered</div>
          ) : (
            skills.map(skill => (
              <div
                key={skill.name}
                className="skill-item"
                onClick={() => handleSkillClick(skill)}
                role="button"
                tabIndex={0}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') handleSkillClick(skill);
                }}
              >
                <div className="skill-name">/{skill.name}</div>
                <div className="skill-description">{skill.description}</div>
                <div className="skill-source">{skill.source}</div>
              </div>
            ))
          )}
        </div>
      )}
    </div>
  );
}
