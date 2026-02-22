import { useEffect, useRef } from 'react';
import type { PaletteItem } from './types';
import { getConversationState } from './sources/ConversationSource';

interface Props {
  results: PaletteItem[];
  selectedIndex: number;
  mode: 'search' | 'action';
  onHover: (index: number) => void;
  onClick: (index: number) => void;
}

export function CommandPaletteResults({ results, selectedIndex, mode, onHover, onClick }: Props) {
  const listRef = useRef<HTMLDivElement>(null);

  // Scroll selected item into view
  useEffect(() => {
    if (!listRef.current) return;
    const selected = listRef.current.querySelector('.cp-result.selected');
    selected?.scrollIntoView({ block: 'nearest' });
  }, [selectedIndex]);

  if (results.length === 0) {
    return (
      <div className="cp-results-empty">
        {mode === 'action' ? 'No matching commands' : 'No results'}
      </div>
    );
  }

  // Group results by category
  const groups: { category: string; items: { item: PaletteItem; globalIndex: number }[] }[] = [];
  let currentGroup: typeof groups[0] | null = null;

  results.forEach((item, index) => {
    if (!currentGroup || currentGroup.category !== item.category) {
      currentGroup = { category: item.category, items: [] };
      groups.push(currentGroup);
    }
    currentGroup.items.push({ item, globalIndex: index });
  });

  return (
    <div className="cp-results" ref={listRef}>
      {groups.map(group => (
        <div key={group.category} className="cp-result-group">
          <div className="cp-result-category">{group.category}</div>
          {group.items.map(({ item, globalIndex }) => (
            <button
              key={item.id}
              className={`cp-result ${globalIndex === selectedIndex ? 'selected' : ''}`}
              onClick={() => onClick(globalIndex)}
              onMouseMove={() => onHover(globalIndex)}
            >
              {mode === 'search' && item.category === 'Conversations' && (
                <span className={`conv-state-dot ${getConversationState(item)}`} />
              )}
              <div className="cp-result-text">
                <span className="cp-result-title">{item.title}</span>
                {item.subtitle && mode !== 'action' && (
                  <span className="cp-result-subtitle">{item.subtitle}</span>
                )}
              </div>
              {mode === 'action' && item.subtitle && (
                <span className="cp-shortcut-hint">{item.subtitle}</span>
              )}
            </button>
          ))}
        </div>
      ))}
    </div>
  );
}
