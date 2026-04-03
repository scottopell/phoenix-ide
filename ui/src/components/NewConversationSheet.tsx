import { useState, useEffect, useRef, useCallback, KeyboardEvent } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api';
import { LlmStatusBanner } from './LlmStatusBanner';
import { SettingsFields } from './SettingsFields';
import type { DirStatus } from './SettingsFields';
import { generateUUID } from '../utils/uuid';
import type { ModelsResponse } from '../api';

const LAST_CWD_KEY = 'phoenix-last-cwd';
const LAST_MODEL_KEY = 'phoenix-last-model';

interface NewConversationSheetProps {
  isOpen: boolean;
  onClose: () => void;
}

export function NewConversationSheet({ isOpen, onClose }: NewConversationSheetProps) {
  const navigate = useNavigate();
  const sheetRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const dragStartY = useRef<number | null>(null);
  const dragCurrentY = useRef<number | null>(null);
  const [translateY, setTranslateY] = useState(0);
  const [isAnimating, setIsAnimating] = useState(false);

  const [cwd, setCwd] = useState(() => localStorage.getItem(LAST_CWD_KEY) || '');
  const [dirStatus, setDirStatus] = useState<DirStatus>('checking');
  const [models, setModels] = useState<ModelsResponse | null>(null);
  const [selectedModel, setSelectedModel] = useState<string | null>(() => localStorage.getItem(LAST_MODEL_KEY));
  const [draft, setDraft] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [showAllModels, setShowAllModels] = useState(false);

  // Load models & env on open
  useEffect(() => {
    if (!isOpen) return;
    api.listModels().then(modelsData => {
      setModels(modelsData);
      if (!selectedModel) setSelectedModel(modelsData.default);
    }).catch(console.error);
    api.getEnv().then(env => {
      if (!localStorage.getItem(LAST_CWD_KEY)) setCwd(env.home_dir);
    }).catch(console.error);
  }, [isOpen, selectedModel]);

  // Focus textarea when opened
  useEffect(() => {
    if (isOpen) {
      setTimeout(() => textareaRef.current?.focus(), 300);
    } else {
      // Reset state when closing
      setDraft('');
      setError(null);
      setCreating(false);
      setTranslateY(0);
    }
  }, [isOpen]);

  // Persist preferences
  useEffect(() => { localStorage.setItem(LAST_CWD_KEY, cwd); }, [cwd]);
  useEffect(() => { if (selectedModel) localStorage.setItem(LAST_MODEL_KEY, selectedModel); }, [selectedModel]);

  // Close on Escape
  useEffect(() => {
    if (!isOpen) return;
    const handleKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', handleKey);
    return () => window.removeEventListener('keydown', handleKey);
  }, [isOpen, onClose]);

  // Swipe-to-dismiss
  const handleTouchStart = useCallback((e: React.TouchEvent) => {
    const touch = e.touches[0];
    if (touch) {
      dragStartY.current = touch.clientY;
      dragCurrentY.current = touch.clientY;
    }
  }, []);

  const handleTouchMove = useCallback((e: React.TouchEvent) => {
    if (dragStartY.current === null) return;
    const touch = e.touches[0];
    if (!touch) return;
    dragCurrentY.current = touch.clientY;
    const delta = touch.clientY - dragStartY.current;
    if (delta > 0) {
      setTranslateY(delta);
    }
  }, []);

  const handleTouchEnd = useCallback(() => {
    if (dragStartY.current === null || dragCurrentY.current === null) return;
    const delta = dragCurrentY.current - dragStartY.current;
    dragStartY.current = null;
    dragCurrentY.current = null;
    if (delta > 100) {
      // Dismiss
      setIsAnimating(true);
      setTranslateY(window.innerHeight);
      setTimeout(() => {
        setIsAnimating(false);
        onClose();
      }, 250);
    } else {
      setIsAnimating(true);
      setTranslateY(0);
      setTimeout(() => setIsAnimating(false), 250);
    }
  }, [onClose]);

  const handleSend = async () => {
    const trimmed = draft.trim();
    if (!trimmed) return;
    if (creating || dirStatus === 'invalid' || dirStatus === 'checking') return;

    setError(null);
    setCreating(true);

    try {
      if (dirStatus === 'will-create') {
        const mkdirResult = await api.mkdir(cwd.trim());
        if (!mkdirResult.created) {
          setError(mkdirResult.error || 'Failed to create directory');
          setCreating(false);
          return;
        }
      }
      const messageId = generateUUID();
      const conv = await api.createConversation(cwd.trim(), trimmed, messageId, selectedModel || undefined, []);
      onClose();
      navigate(`/c/${conv.slug}`);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create conversation');
      setCreating(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  if (!isOpen) return null;

  const canSend = draft.trim().length > 0 && !creating && dirStatus !== 'invalid' && dirStatus !== 'checking';
  const buttonText = creating ? 'Creating...' : 'Send';
  const settingsProps = { cwd, setCwd, dirStatus, onDirStatusChange: setDirStatus, selectedModel, setSelectedModel, models, showAllModels, setShowAllModels };

  return (
    <div className="bottom-sheet-overlay" onClick={onClose}>
      <div
        ref={sheetRef}
        className={`bottom-sheet ${isAnimating ? 'animating' : ''}`}
        style={{ transform: `translateY(${translateY}px)` }}
        onClick={(e) => e.stopPropagation()}
      >
        <div
          className="bottom-sheet-handle"
          onTouchStart={handleTouchStart}
          onTouchMove={handleTouchMove}
          onTouchEnd={handleTouchEnd}
        >
          <div className="bottom-sheet-handle-bar" />
        </div>
        <div className="bottom-sheet-content">
          <h2 className="bottom-sheet-title">New conversation</h2>
          <LlmStatusBanner models={models} />
          {error && <div className="new-conv-error">{error}</div>}
          <div className="bottom-sheet-settings">
            <SettingsFields {...settingsProps} />
          </div>
          <textarea
            ref={textareaRef}
            className="bottom-sheet-textarea"
            placeholder="What would you like to work on?"
            rows={3}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={handleKeyDown}
            disabled={creating}
          />
          <button className="new-conv-send bottom-sheet-send" onClick={handleSend} disabled={!canSend}>
            {buttonText}
          </button>
        </div>
      </div>
    </div>
  );
}
