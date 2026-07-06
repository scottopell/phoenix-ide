import { useState, useEffect, useRef } from 'react';

interface RenameDialogProps {
  visible: boolean;
  currentName: string;
  onRename: (newName: string) => void;
  onGenerate?: () => Promise<void>;
  onCancel: () => void;
  error: string | undefined;
}

export function RenameDialog({
  visible,
  currentName,
  onRename,
  onGenerate,
  onCancel,
  error,
}: RenameDialogProps) {
  const [name, setName] = useState(currentName);
  const [generating, setGenerating] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (visible) {
      setName(currentName);
      setGenerating(false);
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

  const handleGenerate = async () => {
    if (!onGenerate || generating) return;
    setGenerating(true);
    try {
      await onGenerate();
    } catch {
      // Caller owns the displayed error via the existing `error` prop.
    } finally {
      setGenerating(false);
    }
  };

  const isValid = name.trim().length > 0 && /^[a-z0-9-]+$/.test(name.trim());

  if (!visible) return null;

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div
        className="modal rename-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="rename-dialog-title"
        onClick={(e) => e.stopPropagation()}
      >
        <h3 id="rename-dialog-title">Rename Conversation</h3>
        <form onSubmit={handleSubmit}>
          <input
            ref={inputRef}
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value.toLowerCase().replace(/[^a-z0-9-]/g, '-'))}
            placeholder="conversation-name"
            className="rename-input"
            disabled={generating}
          />
          {error && <p className="error-text">{error}</p>}
          {!isValid && name.trim() && (
            <p className="help-text">Use lowercase letters, numbers, and hyphens only</p>
          )}
          {onGenerate && (
            <button
              type="button"
              className="btn-secondary rename-generate-btn"
              onClick={() => void handleGenerate()}
              disabled={generating}
              aria-busy={generating}
            >
              {generating ? 'Generating…' : 'Generate with AI'}
            </button>
          )}
          <div className="modal-actions">
            <button type="button" className="btn-secondary" onClick={onCancel}>
              Cancel
            </button>
            <button
              type="submit"
              className="btn-primary"
              disabled={generating || !isValid || name.trim() === currentName}
            >
              Rename
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
