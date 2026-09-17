import { useState, useEffect, useRef } from 'react';
import { useRegisterFocusScope } from '../hooks/useFocusScope';

interface RenameDialogProps {
  visible: boolean;
  currentName: string;
  onRename: (newName: string) => void | Promise<void>;
  onGenerate?: () => Promise<void>;
  onCancel: () => void;
  error: string | undefined;
  normalizeInput?: (value: string) => string;
  isValidName?: (value: string) => boolean;
  helpText?: string;
  maxLength?: number;
}

export function RenameDialog({
  visible,
  currentName,
  onRename,
  onGenerate,
  onCancel,
  error,
  normalizeInput = (value) => value.toLowerCase().replace(/[^a-z0-9-]/g, '-'),
  isValidName = (value) => /^[a-z0-9-]+$/.test(value),
  helpText = 'Use lowercase letters, numbers, and hyphens only',
  maxLength,
}: RenameDialogProps) {
  const [name, setName] = useState(currentName);
  const [generating, setGenerating] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  useRegisterFocusScope(visible ? 'rename-conversation' : null);

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
        if (e.key === 'Escape' && !generating) onCancel();
      };
      document.addEventListener('keydown', handleEscape);
      return () => document.removeEventListener('keydown', handleEscape);
    }
    return undefined;
  }, [visible, generating, onCancel]);

  const handleCancel = () => {
    if (!generating) onCancel();
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (generating) return;
    const trimmed = name.trim();
    if (trimmed && trimmed !== currentName) {
      setGenerating(true);
      try {
        await onRename(trimmed);
      } finally {
        setGenerating(false);
      }
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

  const isValid = name.trim().length > 0 && isValidName(name.trim());

  if (!visible) return null;

  return (
    <div className="modal-overlay" onClick={handleCancel}>
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
            onChange={(e) => setName(normalizeInput(e.target.value))}
            placeholder="conversation-name"
            className="rename-input"
            disabled={generating}
            maxLength={maxLength}
          />
          {error && <p className="error-text">{error}</p>}
          {!isValid && name.trim() && (
            <p className="help-text">{helpText}</p>
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
            <button type="button" className="btn-secondary" onClick={handleCancel} disabled={generating}>
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
