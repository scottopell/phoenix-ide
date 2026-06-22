import { useState, useEffect, useMemo } from 'react';
import { api } from '../api';
import type { SkillEntry } from '../api';
import { GroundingSection, GroundingState } from './GroundingPanel';
import { summarizeSkills, groupSkills } from './groundingSummaries';
import './SkillsPanel.css';

interface SkillsPanelProps {
  conversationId: string | undefined;
  onSkillClick?: (skill: SkillEntry) => void;
  expanded?: boolean;
  onToggleExpanded?: (expanded: boolean) => void;
  expandedGroups?: Set<string> | null;
  onExpandedGroupsChange?: (groups: Set<string>) => void;
  scrollTop?: number;
  onScrollTopChange?: (scrollTop: number) => void;
}

export function SkillsPanel({
  conversationId,
  onSkillClick,
  expanded: controlledExpanded,
  onToggleExpanded,
  expandedGroups: controlledExpandedGroups,
  onExpandedGroupsChange,
  scrollTop,
  onScrollTopChange,
}: SkillsPanelProps) {
  const [skills, setSkills] = useState<SkillEntry[]>([]);
  const [internalExpanded, setInternalExpanded] = useState(false);
  const expanded = controlledExpanded ?? internalExpanded;
  const setExpanded = onToggleExpanded ?? setInternalExpanded;
  const [internalExpandedGroups, setInternalExpandedGroups] = useState<Set<string>>(new Set());
  const expandedGroups = controlledExpandedGroups ?? internalExpandedGroups;
  const setExpandedGroups = onExpandedGroupsChange ?? setInternalExpandedGroups;
  const hasControlledGroups = controlledExpandedGroups !== undefined && controlledExpandedGroups !== null;

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
          const groups = groupSkills(resp.skills);
          if (!hasControlledGroups) setExpandedGroups(new Set(groups.keys()));
        }
      })
      .catch(() => {
        if (!cancelled) setSkills([]);
      });


  return () => {
      cancelled = true;
      controller.abort();
    };
  }, [conversationId, hasControlledGroups, setExpandedGroups]);

  const grouped = useMemo(() => groupSkills(skills), [skills]);

  const handleSkillClick = (skill: SkillEntry) => {
    if (onSkillClick) {
      onSkillClick(skill);
    }
  };

  const toggleGroup = (group: string) => {
    const next = new Set(expandedGroups);
    if (next.has(group)) {
      next.delete(group);
    } else {
      next.add(group);
    }
    setExpandedGroups(next);
  };

  return (
    <GroundingSection
      icon="/"
      title="Skills"
      summary={summarizeSkills(skills)}
      count={skills.length}
      expanded={expanded}
      onToggle={() => setExpanded(!expanded)}
      scrollTop={scrollTop}
      onScrollTopChange={onScrollTopChange}
    >
      <div className={`skills-panel${expanded ? ' is-expanded' : ''}`}>
        {skills.length === 0 ? (
          <GroundingState>No skills discovered for this conversation.</GroundingState>
        ) : (
          <div className="skills-panel-body">
            {Array.from(grouped.entries()).map(([group, items]) => (
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
            ))}
          </div>
        )}
      </div>
    </GroundingSection>
  );
}
