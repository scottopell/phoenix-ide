import { useState, useEffect, useRef } from 'react';

interface RenameDialogProps {
  visible: boolean;
  currentName: string;
  onRename: (newName: string) => void;
  onCancel: () => void;
  error: string | undefined;
}

export function RenameDialog({
  visible,
  currentName,
  onRename,
  onCancel,
  error,
}: RenameDialogProps) {
  const [name, setName] = useState(currentName);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (visible) {
      setName(currentName);
      // Focus and select input after a short delay to ensure modal is rendered
      setTimeout(() => inputRef.current?.select(), 50);
    }
  }, [visible, currentName]);

  useEffect(() => {
    if (visible) {
      const handleEscape = (e: KeyboardEvent) => {
        if (e.key === 'Escape') onCancel();
      };
      document.addEventListener('keydown', handleEscape);
      return () => document.removeEventListener('keydown', handleEscape);
    }
    return undefined;
  }, [visible, onCancel]);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = name.trim();
    if (trimmed && trimmed !== currentName) {
      onRename(trimmed);
    }
  };

  const isValid = name.trim().length > 0 && /^[a-z0-9-]+$/.test(name.trim());

  if (!visible) return null;

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal rename-dialog" onClick={(e) => e.stopPropagation()}>
        <h3>Rename Conversation</h3>
        <form onSubmit={handleSubmit}>
          <input
            ref={inputRef}
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value.toLowerCase().replace(/[^a-z0-9-]/g, '-'))}
            placeholder="conversation-name"
            className="rename-input"
          />
          {error && <p className="error-text">{error}</p>}
          {!isValid && name.trim() && (
            <p className="help-text">Use lowercase letters, numbers, and hyphens only</p>
          )}
          <div className="modal-actions">
            <button type="button" className="btn-secondary" onClick={onCancel}>
              Cancel
            </button>
            <button
              type="submit"
              className="btn-primary"
              disabled={!isValid || name.trim() === currentName}
            >
              Rename
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
