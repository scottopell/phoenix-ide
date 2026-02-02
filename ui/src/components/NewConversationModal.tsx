import { useState, KeyboardEvent } from 'react';
import { api } from '../api';

interface NewConversationModalProps {
  visible: boolean;
  onClose: () => void;
  onCreated: (conv: { id: string; slug: string }) => void;
}

export function NewConversationModal({ visible, onClose, onCreated }: NewConversationModalProps) {
  const [cwd, setCwd] = useState('/home/exedev');
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  if (!visible) return null;

  const handleCreate = async () => {
    const trimmed = cwd.trim();
    if (!trimmed) {
      setError('Please enter a directory');
      return;
    }

    setError(null);
    setCreating(true);

    try {
      // Validate
      const validation = await api.validateCwd(trimmed);
      if (!validation.valid) {
        setError(validation.error || 'Invalid directory');
        setCreating(false);
        return;
      }

      // Create
      const conv = await api.createConversation(trimmed);
      onCreated(conv);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create conversation');
    } finally {
      setCreating(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      handleCreate();
    }
  };

  const handleOverlayClick = (e: React.MouseEvent) => {
    if (e.target === e.currentTarget) {
      onClose();
    }
  };

  return (
    <div id="modal-overlay" onClick={handleOverlayClick}>
      <div id="new-conv-modal" className="modal">
        <h3>New Conversation</h3>
        <label htmlFor="cwd-input">Working Directory</label>
        <input
          type="text"
          id="cwd-input"
          placeholder="/home/exedev"
          value={cwd}
          onChange={(e) => setCwd(e.target.value)}
          onKeyDown={handleKeyDown}
          autoFocus
        />
        {error && (
          <div id="cwd-error" className="error">
            {error}
          </div>
        )}
        <div className="modal-actions">
          <button id="modal-cancel" className="btn-secondary" onClick={onClose} disabled={creating}>
            Cancel
          </button>
          <button id="modal-create" className="btn-primary" onClick={handleCreate} disabled={creating}>
            {creating ? 'Creating...' : 'Create'}
          </button>
        </div>
      </div>
    </div>
  );
}
