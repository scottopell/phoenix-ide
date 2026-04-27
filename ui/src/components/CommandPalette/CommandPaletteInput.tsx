import { useRef, useEffect } from 'react';

interface Props {
  value: string;
  mode: 'search' | 'action';
  hasActiveConversation: boolean;
  onChange: (value: string) => void;
  onKeyDown: (e: React.KeyboardEvent) => void;
}

export function CommandPaletteInput({ value, mode, hasActiveConversation, onChange, onKeyDown }: Props) {
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const placeholder = mode === 'action'
    ? 'Type a command...'
    : hasActiveConversation
      ? 'Search conversations and files...'
      : 'Search conversations...';

  // In action mode, show > as styled indicator, strip from input
  const displayValue = mode === 'action' && value.startsWith('>') ? value.slice(1) : value;

  const handleChange = (newDisplayValue: string) => {
    if (mode === 'action') {
      onChange('>' + newDisplayValue);
    } else {
      onChange(newDisplayValue);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    // Backspace on empty action mode input → switch back to search mode
    if (e.key === 'Backspace' && mode === 'action' && displayValue === '') {
      e.preventDefault();
      onChange('');
      return;
    }
    onKeyDown(e);
  };

  return (
    <div className="cp-input-wrap">
      {mode === 'action' && <span className="cp-mode-indicator">&gt;</span>}
      <input
        ref={inputRef}
        className="cp-input"
        type="text"
        value={displayValue}
        placeholder={placeholder}
        onChange={e => handleChange(e.target.value)}
        onKeyDown={handleKeyDown}
        spellCheck={false}
        autoComplete="off"
      />
    </div>
  );
}
