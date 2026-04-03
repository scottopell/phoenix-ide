import { useState, useEffect, useRef, KeyboardEvent, ClipboardEvent, ChangeEvent } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api';
import { ImageAttachments } from '../components/ImageAttachments';
import { LlmStatusBanner } from '../components/LlmStatusBanner';
import { SettingsFields, DIR_STATUS_CONFIG } from '../components/SettingsFields';
import type { DirStatus } from '../components/SettingsFields';
import { VoiceRecorder, isWebSpeechSupported } from '../components/VoiceInput';
import { SUPPORTED_IMAGE_TYPES, processImageFiles } from '../utils/images';
import { generateUUID } from '../utils/uuid';
import type { ImageData, ModelsResponse } from '../api';

const LAST_CWD_KEY = 'phoenix-last-cwd';
const LAST_MODEL_KEY = 'phoenix-last-model';
const RECENT_DIRS_KEY = 'phoenix-recent-dirs';
const MAX_RECENT = 5;

function getRecentDirs(): string[] {
  try {
    return JSON.parse(localStorage.getItem(RECENT_DIRS_KEY) || '[]');
  } catch { return []; }
}

function addRecentDir(dir: string) {
  const recent = getRecentDirs().filter(d => d !== dir);
  recent.unshift(dir);
  localStorage.setItem(RECENT_DIRS_KEY, JSON.stringify(recent.slice(0, MAX_RECENT)));
}

interface NewConversationPageProps {
  desktopMode?: boolean;
}

export function NewConversationPage({ desktopMode }: NewConversationPageProps = {}) {
  const navigate = useNavigate();
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const [homeDir, setHomeDir] = useState<string>('');
  const [cwd, setCwd] = useState(() => localStorage.getItem(LAST_CWD_KEY) || '');
  const [dirStatus, setDirStatus] = useState<DirStatus>(() =>
    localStorage.getItem(LAST_CWD_KEY) ? 'exists' : 'checking'
  );
  const [models, setModels] = useState<ModelsResponse | null>(null);
  const [selectedModel, setSelectedModel] = useState<string | null>(() => localStorage.getItem(LAST_MODEL_KEY));
  const [draft, setDraft] = useState('');
  const [images, setImages] = useState<ImageData[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [showAllModels, setShowAllModels] = useState(false);

  const [recentDirs, setRecentDirs] = useState<string[]>(() => getRecentDirs());

  const voiceSupported = isWebSpeechSupported();
  const [interimText, setInterimText] = useState('');
  const draftBeforeVoiceRef = useRef<string>('');

  // Load models and environment info
  useEffect(() => {
    api.listModels().then(modelsData => {
      setModels(modelsData);
      if (!selectedModel) setSelectedModel(modelsData.default);
    }).catch(console.error);
    api.getEnv().then(env => {
      setHomeDir(env.home_dir);
      // Only set default cwd if nothing was saved in localStorage
      if (!localStorage.getItem(LAST_CWD_KEY)) {
        setCwd(env.home_dir);
      }
    }).catch(console.error);
  }, [selectedModel]);

  // Directory validation is handled by DirectoryPicker via onDirStatusChange

  // Save preferences
  useEffect(() => { localStorage.setItem(LAST_CWD_KEY, cwd); }, [cwd]);
  useEffect(() => { if (selectedModel) localStorage.setItem(LAST_MODEL_KEY, selectedModel); }, [selectedModel]);

  // Auto-resize textarea
  useEffect(() => {
    const ta = textareaRef.current;
    if (ta) {
      ta.style.height = 'auto';
      ta.style.height = Math.min(ta.scrollHeight, 200) + 'px';
    }
  }, [draft]);

  // Focus textarea on mount
  useEffect(() => { textareaRef.current?.focus(); }, []);

  const addImages = async (files: File[]) => {
    try {
      const newImages = await processImageFiles(files);
      setImages([...images, ...newImages]);
    } catch (err) {
      console.error('Error processing images:', err);
    }
  };

  const handlePaste = async (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const items = e.clipboardData?.items;
    if (!items) return;
    const imageFiles: File[] = [];
    for (const item of items) {
      if (item.type.startsWith('image/')) {
        const file = item.getAsFile();
        if (file) imageFiles.push(file);
      }
    }
    if (imageFiles.length > 0) {
      e.preventDefault();
      await addImages(imageFiles);
    }
  };

  const handleFileChange = async (e: ChangeEvent<HTMLInputElement>) => {
    const files = Array.from(e.target.files || []);
    if (files.length > 0) await addImages(files);
    e.target.value = '';
  };

  const [bgToast, setBgToast] = useState<string | null>(null);

  const handleSend = async (background = false) => {
    const trimmed = draft.trim();
    if (!trimmed && images.length === 0) return;
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
      const trimmedCwd = cwd.trim();
      const conv = await api.createConversation(
        trimmedCwd, trimmed, messageId, selectedModel || undefined, images
      );
      addRecentDir(trimmedCwd);
      setRecentDirs(getRecentDirs());
      if (background) {
        // Stay on page, reset form, show toast
        setDraft('');
        setImages([]);
        setCreating(false);
        setBgToast(`Started: ${conv.slug}`);
        setTimeout(() => setBgToast(null), 4000);
      } else {
        navigate(`/c/${conv.slug}`);
      }
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

  const handleVoiceFinal = (text: string) => {
    if (!text) return;
    setInterimText('');
    const baseDraft = draftBeforeVoiceRef.current || draft;
    const newDraft = baseDraft.trim() ? baseDraft.trimEnd() + ' ' + text : text;
    setDraft(newDraft);
    draftBeforeVoiceRef.current = newDraft;
  };

  const handleVoiceInterim = (text: string) => {
    if (!interimText && text) draftBeforeVoiceRef.current = draft;
    setInterimText(text);
  };

  const handleRemoveImage = (index: number) => {
    setImages(images.filter((_, idx) => idx !== index));
  };

  const hasContent = draft.trim().length > 0 || images.length > 0;
  const canSend = hasContent && !creating && dirStatus !== 'invalid' && dirStatus !== 'checking';

  const { icon: dirStatusIcon, class: dirStatusClass } = DIR_STATUS_CONFIG[dirStatus];
  const cwdDisplay = (homeDir && cwd.trim().startsWith(homeDir))
    ? '~/' + cwd.trim().slice(homeDir.length).replace(/^\//, '')
    : cwd.trim() || '~/';
  const modelDisplay = models?.models.find(m => m.id === selectedModel)?.id.replace(/-sonnet|-opus/g, '') || '...';
  const buttonText = creating ? (dirStatus === 'will-create' ? 'Creating folder...' : 'Creating...') : 'Send';
  const textareaValue = interimText ? (draft.trim() ? draft.trimEnd() + ' ' + interimText : interimText) : draft;

  const settingsProps = { cwd, setCwd, dirStatus, onDirStatusChange: setDirStatus, selectedModel, setSelectedModel, models, showAllModels, setShowAllModels };

  return (
    <div className="new-conv-page">
      <input
        ref={fileInputRef}
        type="file"
        accept={SUPPORTED_IMAGE_TYPES.join(',')}
        multiple
        onChange={handleFileChange}
        style={{ display: 'none' }}
      />

      {!desktopMode && (
        <header className="new-conv-header-minimal">
          <button className="back-link" onClick={() => navigate('/')}>← Back</button>
        </header>
      )}

      <main className="new-conv-main">
        {/* Desktop: workbench card */}
        <div className="new-conv-card desktop-only">
          <LlmStatusBanner models={models} />
          {error && <div className="new-conv-error">{error}</div>}

          {/* Recent projects */}
          {recentDirs.length > 0 && (
            <div className="new-conv-recent">
              {recentDirs.map(dir => {
                const label = dir.split('/').filter(Boolean).pop() || dir;
                const isSelected = cwd.trim() === dir;
                return (
                  <button
                    key={dir}
                    className={`new-conv-recent-chip ${isSelected ? 'active' : ''}`}
                    onClick={() => setCwd(dir)}
                    title={dir}
                  >
                    {label}
                  </button>
                );
              })}
            </div>
          )}

          {/* Main input */}
          <ImageAttachments images={images} onRemove={handleRemoveImage} />
          <textarea
            ref={textareaRef}
            className="new-conv-textarea"
            placeholder="What would you like to work on?"
            rows={3}
            value={textareaValue}
            onChange={(e) => {
              setDraft(e.target.value);
              if (interimText) { setInterimText(''); draftBeforeVoiceRef.current = ''; }
            }}
            onKeyDown={handleKeyDown}
            onPaste={handlePaste}
            disabled={creating}
          />

          {/* Actions row: settings chips + send */}
          <div className="new-conv-actions">
            <div className="new-conv-chips">
              <button className="new-conv-chip" title={cwd} onClick={() => setShowSettings(true)}>
                <span className={`chip-status ${dirStatusClass}`}>{dirStatusIcon}</span>
                {cwdDisplay}
              </button>
              <button className="new-conv-chip" onClick={() => setShowSettings(true)}>
                {modelDisplay}
              </button>
            </div>
            <div className="new-conv-send-group">
              <button className="icon-btn" onClick={() => fileInputRef.current?.click()} title="Attach image" disabled={creating}>📎</button>
              {voiceSupported && <VoiceRecorder onSpeech={handleVoiceFinal} onInterim={handleVoiceInterim} disabled={creating} />}
              <button className="new-conv-send" onClick={() => handleSend(false)} disabled={!canSend}>{buttonText}</button>
            </div>
          </div>

          {/* Expanded settings */}
          {showSettings && (
            <div className="new-conv-settings-expanded">
              <SettingsFields {...settingsProps} />
            </div>
          )}
        </div>

        {/* Mobile: keep existing layout */}
        <div className="new-conv-content mobile-only">
          <LlmStatusBanner models={models} />
          {error && <div className="new-conv-error">{error}</div>}

          <div className="new-conv-settings-card">
            <SettingsFields {...settingsProps} />
          </div>
        </div>
      </main>

      {/* Mobile: bottom-anchored input */}
      <div className="new-conv-bottom-input mobile-only">
        <ImageAttachments images={images} onRemove={handleRemoveImage} />
        <textarea
          className="new-conv-textarea-mobile"
          placeholder="What would you like to work on?"
          rows={2}
          value={textareaValue}
          onChange={(e) => {
            setDraft(e.target.value);
            if (interimText) { setInterimText(''); draftBeforeVoiceRef.current = ''; }
          }}
          onKeyDown={handleKeyDown}
          onPaste={handlePaste}
          disabled={creating}
        />
        <div className="new-conv-input-row">
          <div className="new-conv-input-left">
            <button className="icon-btn" onClick={() => fileInputRef.current?.click()} title="Attach image" disabled={creating}>📎</button>
            {voiceSupported && <VoiceRecorder onSpeech={handleVoiceFinal} onInterim={handleVoiceInterim} disabled={creating} />}
          </div>
          <button className="new-conv-send" onClick={() => handleSend(false)} disabled={!canSend}>{buttonText}</button>
        </div>
      </div>
      {bgToast && <div className="bg-toast">{bgToast}</div>}
    </div>
  );
}
