import { useState, useEffect, useRef, KeyboardEvent } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api';
import { SettingsFields, DIR_STATUS_CONFIG } from './SettingsFields';
import type { DirStatus } from './SettingsFields';
import { generateUUID } from '../utils/uuid';
import type { ModelsResponse } from '../api';

const LAST_CWD_KEY = 'phoenix-last-cwd';
const LAST_MODEL_KEY = 'phoenix-last-model';

interface SidebarNewFormProps {
  onClose: () => void;
  onCreated: () => void;
  showToast: (message: string, duration?: number) => void;
}

export function SidebarNewForm({ onClose, onCreated, showToast }: SidebarNewFormProps) {
  const navigate = useNavigate();
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const [cwd, setCwd] = useState(() => localStorage.getItem(LAST_CWD_KEY) || '');
  const [dirStatus, setDirStatus] = useState<DirStatus>('checking');
  const [models, setModels] = useState<ModelsResponse | null>(null);
  const [selectedModel, setSelectedModel] = useState<string | null>(() => localStorage.getItem(LAST_MODEL_KEY));
  const [draft, setDraft] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [showAllModels, setShowAllModels] = useState(false);

  useEffect(() => {
    api.listModels().then(modelsData => {
      setModels(modelsData);
      if (!selectedModel) setSelectedModel(modelsData.default);
    }).catch(console.error);
    api.getEnv().then(env => {
      if (!localStorage.getItem(LAST_CWD_KEY)) setCwd(env.home_dir);
    }).catch(console.error);
    setTimeout(() => textareaRef.current?.focus(), 100);
  }, [selectedModel]);

  useEffect(() => {
    const trimmed = cwd.trim();
    if (!trimmed || !trimmed.startsWith('/')) { setDirStatus('invalid'); return; }
    setDirStatus('checking');
    const t = setTimeout(async () => {
      try {
        const v = await api.validateCwd(trimmed);
        if (v.valid) { setDirStatus('exists'); }
        else {
          const p = trimmed.substring(0, trimmed.lastIndexOf('/')) || '/';
          const pv = await api.validateCwd(p);
          setDirStatus(pv.valid ? 'will-create' : 'invalid');
        }
      } catch { setDirStatus('invalid'); }
    }, 300);
    return () => clearTimeout(t);
  }, [cwd]);

  useEffect(() => { localStorage.setItem(LAST_CWD_KEY, cwd); }, [cwd]);
  useEffect(() => { if (selectedModel) localStorage.setItem(LAST_MODEL_KEY, selectedModel); }, [selectedModel]);

  // Escape to close
  useEffect(() => {
    const handler = (e: globalThis.KeyboardEvent) => {
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation(); onClose(); }
    };
    window.addEventListener('keydown', handler, true);
    return () => window.removeEventListener('keydown', handler, true);
  }, [onClose]);

  const doCreate = async (background: boolean) => {
    const trimmed = draft.trim();
    if (!trimmed || creating || dirStatus === 'invalid' || dirStatus === 'checking') return;
    setError(null);
    setCreating(true);
    try {
      if (dirStatus === 'will-create') {
        const r = await api.mkdir(cwd.trim());
        if (!r.created) { setError(r.error || 'Failed to create directory'); setCreating(false); return; }
      }
      const messageId = generateUUID();
      const conv = await api.createConversation(cwd.trim(), trimmed, messageId, selectedModel || undefined, []);
      onCreated();
      if (background) {
        showToast(`Started: ${conv.slug}`, 4000);
      } else {
        navigate(`/c/${conv.slug}`);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create conversation');
      setCreating(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); doCreate(false); }
  };

  const { class: dirStatusClass } = DIR_STATUS_CONFIG[dirStatus];
  const canSend = draft.trim().length > 0 && !creating && dirStatus !== 'invalid' && dirStatus !== 'checking';
  const settingsProps = { cwd, setCwd, dirStatus, dirStatusClass, selectedModel, setSelectedModel, models, showAllModels, setShowAllModels };

  return (
    <div className="sidebar-new-form">
      <div className="sidebar-new-form-header">
        <span className="sidebar-new-form-title">New conversation</span>
        <button className="sidebar-new-form-close" onClick={onClose}>×</button>
      </div>
      {error && <div className="new-conv-error" style={{ margin: '0 0 8px', fontSize: 13 }}>{error}</div>}
      <div className="sidebar-new-form-fields">
        <SettingsFields {...settingsProps} />
      </div>
      <textarea
        ref={textareaRef}
        className="sidebar-new-form-textarea"
        placeholder="What would you like to work on?"
        rows={2}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={handleKeyDown}
        disabled={creating}
      />
      <div className="sidebar-new-form-actions">
        <button
          className="btn-secondary sidebar-new-form-bg"
          onClick={() => doCreate(true)}
          disabled={!canSend}
        >
          Background
        </button>
        <button
          className="btn-primary sidebar-new-form-send"
          onClick={() => doCreate(false)}
          disabled={!canSend}
        >
          {creating ? 'Creating...' : 'Send'}
        </button>
      </div>
    </div>
  );
}
