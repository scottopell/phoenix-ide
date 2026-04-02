import { useEffect, useRef } from 'react';
import { useRegisterFocusScope } from '../hooks';
import { formatShortcut } from '../utils';
import './ShortcutHelpPanel.css';

interface ShortcutHelpPanelProps {
  visible: boolean;
  onClose: () => void;
}

interface ShortcutEntry {
  key: string;
  description: string;
}

interface ShortcutGroup {
  label: string;
  shortcuts: ShortcutEntry[];
}

const SHORTCUT_GROUPS: ShortcutGroup[] = [
  {
    label: 'Global',
    shortcuts: [
      { key: 'Ctrl+P', description: 'Command palette' },
      { key: '?', description: 'Toggle this help panel' },
      { key: 'Escape', description: 'Close panel / navigate back' },
    ],
  },
  {
    label: 'Question Panel',
    shortcuts: [
      { key: 'Up / Down', description: 'Move between options' },
      { key: 'Enter', description: 'Select option' },
      { key: 'Space', description: 'Select/toggle option' },
      { key: 'Tab', description: 'Next question' },
      { key: 'Shift+Tab', description: 'Previous question' },
      { key: 'n', description: 'Add notes (preview questions)' },
      { key: 'Ctrl+Enter', description: 'Submit answers' },
      { key: 'Escape', description: 'Decline (with confirmation)' },
    ],
  },
  {
    label: 'Conversation',
    shortcuts: [
      { key: '/', description: 'Focus message input' },
      { key: 'Enter', description: 'Send message' },
      { key: 'Shift+Enter', description: 'New line' },
    ],
  },
  {
    label: 'Sidebar',
    shortcuts: [
      { key: 'Up / Down', description: 'Navigate conversations' },
      { key: 'Enter', description: 'Open conversation' },
      { key: 'n', description: 'New conversation' },
    ],
  },
];

/**
 * Inner component that mounts only when visible, so the useRegisterFocusScope
 * hook is called unconditionally within this component's lifecycle.
 */
function ShortcutHelpPanelInner({ onClose }: { onClose: () => void }) {
  useRegisterFocusScope('shortcut-help');
  const panelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [onClose]);

  return (
    <div className="shortcut-help-overlay" onClick={onClose}>
      <div
        ref={panelRef}
        className="shortcut-help-panel"
        role="dialog"
        aria-label="Keyboard Shortcuts"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="shortcut-help-title">Keyboard Shortcuts</div>
        {SHORTCUT_GROUPS.map((group) => (
          <div key={group.label} className="shortcut-help-group">
            <div className="shortcut-help-group-header">{group.label}</div>
            {group.shortcuts.map((shortcut) => (
              <div key={shortcut.key + shortcut.description} className="shortcut-help-row">
                <span className="shortcut-help-row-description">{shortcut.description}</span>
                <span className="shortcut-help-key">{formatShortcut(shortcut.key)}</span>
              </div>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

export function ShortcutHelpPanel({ visible, onClose }: ShortcutHelpPanelProps) {
  if (!visible) return null;
  return <ShortcutHelpPanelInner onClose={onClose} />;
}
