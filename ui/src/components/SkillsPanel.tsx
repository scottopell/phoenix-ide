import { useState, useEffect, useMemo } from 'react';
import { api } from '../api';
import type { SkillEntry } from '../api';
import './SkillsPanel.css';

interface SkillsPanelProps {
  conversationId: string | undefined;
  onSkillClick?: (skill: SkillEntry) => void;
  expanded?: boolean;
  onToggleExpanded?: (expanded: boolean) => void;
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
      const prefix = skillPath.substring(0, idx).replace(/\/$/, '');
      const lastSlash = prefix.lastIndexOf('/');
      const dirName = lastSlash >= 0 ? prefix.substring(lastSlash + 1) : prefix;
      if (!dirName || dirName === '~' || dirName === '') return 'User';
      // Detect home directory: prefix matches common home patterns
      const home = prefix;
      if (/^\/Users\/[^/]+$/.test(home) || /^\/home\/[^/]+$/.test(home) || home === '~') {
        return 'User';
      }
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

export function SkillsPanel({ conversationId, onSkillClick, expanded: controlledExpanded, onToggleExpanded }: SkillsPanelProps) {
  const [skills, setSkills] = useState<SkillEntry[]>([]);
  const [internalExpanded, setInternalExpanded] = useState(false);
  // Use controlled state if provided, otherwise internal
  const expanded = controlledExpanded ?? internalExpanded;
  const setExpanded = onToggleExpanded ?? setInternalExpanded;
  /** Which groups are expanded (all by default once skills load) */
  const [expandedGroups, setExpandedGroups] = useState<Set<string>>(new Set());

  useEffect(() => {
    if (!conversationId) {
      setSkills([]);
      return;
    }

    let cancelled = false;
    const controller = new AbortController();

    api.listConversationSkills(conversationId, controller.signal)
      .then(resp => {
        if (!cancelled) {
          setSkills(resp.skills);
          // Initialize all groups as expanded
          const groups = groupSkills(resp.skills);
          setExpandedGroups(new Set(groups.keys()));
        }
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
    if (onSkillClick) {
      onSkillClick(skill);
    }
  };

  const toggleGroup = (group: string) => {
    setExpandedGroups(prev => {
      const next = new Set(prev);
      if (next.has(group)) {
        next.delete(group);
      } else {
        next.add(group);
      }
      return next;
    });
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
            Array.from(grouped.entries()).map(([group, items]) => (
              <div key={group} className="skill-group">
                <div
                  className="skill-group-header"
                  onClick={() => toggleGroup(group)}
                  role="button"
                  tabIndex={0}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' || e.key === ' ') toggleGroup(group);
                  }}
                >
                  <span className={`skill-group-chevron ${expandedGroups.has(group) ? 'expanded' : ''}`}>&#9654;</span>
                  <span>{group}</span>
                  <span className="skill-group-count">({items.length})</span>
                </div>
                {expandedGroups.has(group) && items.map(skill => (
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
