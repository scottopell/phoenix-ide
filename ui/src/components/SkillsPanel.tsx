import { useState, useEffect, useMemo } from 'react';
import { api } from '../api';
import type { SkillEntry } from '../api';
import { useFileExplorer } from '../hooks/useFileExplorer';
import './SkillsPanel.css';

interface SkillsPanelProps {
  conversationId: string | undefined;
}

/**
 * Extract a human-readable group label from a skill's absolute path.
 *
 * For `/Users/scott/dev/projects/http-health-checker/.claude/skills/build/SKILL.md`
 * the group is `http-health-checker` (the directory above `.claude/skills`).
 *
 * For `~/.claude/skills/spears/SKILL.md` the group is `User`.
 */
function groupLabel(skillPath: string): string {
  // Find the segment before .claude/skills or .agents/skills
  const markers = ['.claude/skills', '.agents/skills'];
  for (const marker of markers) {
    const idx = skillPath.indexOf(marker);
    if (idx !== -1) {
      // Get the path portion before the marker, strip trailing slash
      const prefix = skillPath.substring(0, idx).replace(/\/$/, '');
      const lastSlash = prefix.lastIndexOf('/');
      const dirName = lastSlash >= 0 ? prefix.substring(lastSlash + 1) : prefix;
      // Home directory markers
      if (!dirName || dirName === '~' || dirName === '') return 'User';
      return dirName;
    }
  }
  return 'Other';
}

/** Group skills by their parent project directory */
function groupSkills(skills: SkillEntry[]): Map<string, SkillEntry[]> {
  const groups = new Map<string, SkillEntry[]>();
  for (const skill of skills) {
    const label = groupLabel(skill.path);
    const existing = groups.get(label);
    if (existing) {
      existing.push(skill);
    } else {
      groups.set(label, [skill]);
    }
  }
  return groups;
}

export function SkillsPanel({ conversationId }: SkillsPanelProps) {
  const [skills, setSkills] = useState<SkillEntry[]>([]);
  const [expanded, setExpanded] = useState(false);
  const { openFile } = useFileExplorer();

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

  const grouped = useMemo(() => groupSkills(skills), [skills]);

  if (skills.length === 0 && !expanded) {
    return null;
  }

  const handleSkillClick = (skill: SkillEntry) => {
    const skillDir = skill.path.substring(0, skill.path.lastIndexOf('/'));
    openFile(skill.path, skillDir);
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
            Array.from(grouped.entries()).map(([group, groupSkills]) => (
              <div key={group} className="skill-group">
                <div className="skill-group-header">{group}</div>
                {groupSkills.map(skill => (
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
                  </div>
                ))}
              </div>
            ))
          )}
        </div>
      )}
    </div>
  );
}
