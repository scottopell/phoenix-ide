import { useState, useEffect } from 'react';
import { api, ModelsResponse } from '../api';
import { enhancedApi } from '../enhancedApi';
import { DirectoryPicker } from './DirectoryPicker';

interface NewConversationModalProps {
  visible: boolean;
  onClose: () => void;
  onCreated: (conv: { id: string; slug: string }) => void;
}

export function NewConversationModal({ visible, onClose, onCreated }: NewConversationModalProps) {
  const [cwd, setCwd] = useState('/home/exedev');
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [pathValid, setPathValid] = useState(true);
  const [models, setModels] = useState<ModelsResponse | null>(null);
  const [selectedModel, setSelectedModel] = useState<string | null>(null);

  // Debug logging
  console.log('NewConversationModal render, visible:', visible);

  // Reset state when modal opens
  useEffect(() => {
    if (visible) {
      setCwd('/home/exedev');
      setError(null);
      setCreating(false);
      
      // Load available models
      api.listModels().then(modelsData => {
        setModels(modelsData);
        setSelectedModel(modelsData.default);
      }).catch(err => {
        console.error('Failed to load models:', err);
        setError('Failed to load available models');
      });
    }
  }, [visible]);

  // Validate path whenever it changes
  useEffect(() => {
    const validate = async () => {
      const trimmed = cwd.trim();
      if (!trimmed) {
        setPathValid(false);
        return;
      }
      
      // Check if path exists or parent exists (can create)
      const validation = await api.validateCwd(trimmed);
      if (validation.valid) {
        setPathValid(true);
        setError(null);
        return;
      }
      
      // Check if parent exists (we can create this directory)
      const parentPath = trimmed.substring(0, trimmed.lastIndexOf('/')) || '/';
      const parentValidation = await api.validateCwd(parentPath);
      setPathValid(parentValidation.valid);
      // Don't set error here - DirectoryPicker shows status
      setError(null);
    };
    
    validate();
  }, [cwd]);

  if (!visible) return null;

  const handleCreate = async () => {
    const trimmed = cwd.trim();
    if (!trimmed || !pathValid) {
      setError('Please select a valid directory');
      return;
    }

    setError(null);
    setCreating(true);

    try {
      // Check if we need to create the directory
      const validation = await api.validateCwd(trimmed);
      if (!validation.valid) {
        // Directory doesn't exist - try to create it
        // For now, we'll let the backend handle this or show an error
        // In a full implementation, we'd call a mkdir API
        setError('Directory does not exist. Please create it first or select an existing one.');
        setCreating(false);
        return;
      }

      // Create conversation
      const conv = await enhancedApi.createConversation(trimmed, selectedModel || undefined);
      onCreated(conv);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create conversation');
    } finally {
      setCreating(false);
    }
  };

  const handleOverlayClick = (e: React.MouseEvent) => {
    if (e.target === e.currentTarget) {
      onClose();
    }
  };

  return (
    <div id="modal-overlay" onClick={handleOverlayClick}>
      <div id="new-conv-modal" className="modal modal-large">
        <h3>New Conversation</h3>
        <label>Working Directory</label>
        
        <DirectoryPicker value={cwd} onChange={setCwd} />
        
        <label>Model</label>
        <select 
          value={selectedModel || ''}
          onChange={(e) => setSelectedModel(e.target.value)}
          className="model-select"
          disabled={!models || creating}
        >
          {!models ? (
            <option>Loading models...</option>
          ) : (
            models.models.map(model => (
              <option key={model.id} value={model.id}>
                {model.id} - {model.description}
              </option>
            ))
          )}
        </select>
        
        {error && (
          <div id="cwd-error" className="error">
            {error}
          </div>
        )}
        <div className="modal-actions">
          <button id="modal-cancel" className="btn-secondary" onClick={onClose} disabled={creating}>
            Cancel
          </button>
          <button 
            id="modal-create" 
            className="btn-primary" 
            onClick={handleCreate} 
            disabled={creating || !pathValid}
          >
            {creating ? 'Creating...' : 'Create'}
          </button>
        </div>
      </div>
    </div>
  );
}
