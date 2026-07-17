import { useState, useEffect, useMemo, useRef } from 'react';
import { api } from '../api';
import type { SkillEntry } from '../api';
import { GroundingSection, GroundingState } from './GroundingPanel';
import { summarizeSkills, groupSkills } from './groundingSummaries';
import './SkillsPanel.css';

interface SkillsPanelProps {
  conversationId: string | undefined;
  instructionSnapshotVersion?: number | null | undefined;
  onSkillsRefreshed?: (skills: SkillEntry[]) => void;
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
  instructionSnapshotVersion,
  onSkillsRefreshed,
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
  const requestGenerationRef = useRef(0);
  const hasControlledGroupsRef = useRef(hasControlledGroups);
  const setExpandedGroupsRef = useRef(setExpandedGroups);
  const onSkillsRefreshedRef = useRef(onSkillsRefreshed);
  hasControlledGroupsRef.current = hasControlledGroups;
  setExpandedGroupsRef.current = setExpandedGroups;
  onSkillsRefreshedRef.current = onSkillsRefreshed;

  useEffect(() => {
    const requestGeneration = ++requestGenerationRef.current;
    if (!conversationId) {
      setSkills([]);
      onSkillsRefreshedRef.current?.([]);
      return;
    }

    const controller = new AbortController();

    api.listConversationSkills(conversationId, controller.signal)
      .then(resp => {
        if (requestGeneration !== requestGenerationRef.current) return;
        setSkills(resp.skills);
        onSkillsRefreshedRef.current?.(resp.skills);
        if (!hasControlledGroupsRef.current) {
          setExpandedGroupsRef.current(new Set(groupSkills(resp.skills).keys()));
        }
      })
      .catch(() => {
        if (requestGeneration !== requestGenerationRef.current || controller.signal.aborted) return;
        setSkills([]);
        onSkillsRefreshedRef.current?.([]);
      });

    return () => controller.abort();
  }, [conversationId, instructionSnapshotVersion]);

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
