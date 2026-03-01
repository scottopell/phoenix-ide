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

interface NewConversationPageProps {
  desktopMode?: boolean;
}

export function NewConversationPage({ desktopMode }: NewConversationPageProps = {}) {
  const navigate = useNavigate();
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const [homeDir, setHomeDir] = useState<string>('');
  const [cwd, setCwd] = useState(() => localStorage.getItem(LAST_CWD_KEY) || '');
  const [dirStatus, setDirStatus] = useState<DirStatus>('checking');
  const [models, setModels] = useState<ModelsResponse | null>(null);
  const [selectedModel, setSelectedModel] = useState<string | null>(() => localStorage.getItem(LAST_MODEL_KEY));
  const [draft, setDraft] = useState('');
  const [images, setImages] = useState<ImageData[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [showAllModels, setShowAllModels] = useState(false);

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
      const conv = await api.createConversation(
        cwd.trim(), trimmed, messageId, selectedModel || undefined, images
      );
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
        <div className="new-conv-content">
          <h1 className="new-conv-title">New conversation</h1>

          <LlmStatusBanner models={models} />
          {error && <div className="new-conv-error">{error}</div>}

          {/* Mobile: settings card at top */}
          <div className="new-conv-settings-card mobile-only">
            <SettingsFields {...settingsProps} />
          </div>

          {/* Desktop: centered input box */}
          <div className="new-conv-input-box desktop-only">
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
            
            <div className="new-conv-input-actions">
              <div className="new-conv-input-left">
                <button className="icon-btn" onClick={() => fileInputRef.current?.click()} title="Attach image" disabled={creating}>📎</button>
                {voiceSupported && <VoiceRecorder onSpeech={handleVoiceFinal} onInterim={handleVoiceInterim} disabled={creating} />}
              </div>
              <div className="new-conv-input-right">
                {desktopMode && (
                  <button className="new-conv-send-bg" onClick={() => handleSend(true)} disabled={!canSend} title="Create and stay on this page">Background</button>
                )}
                <button className="new-conv-send" onClick={() => handleSend(false)} disabled={!canSend}>{buttonText}</button>
              </div>
            </div>
          </div>

          {/* Desktop: collapsible settings row */}
          <button className="settings-row desktop-only" onClick={() => setShowSettings(!showSettings)}>
            <span className="settings-item">
              <span className="settings-label">dir</span>
              <span className={`settings-status ${dirStatusClass}`}>{dirStatusIcon}</span>
              <span className="settings-value">{cwdDisplay}</span>
            </span>
            <span className="settings-dot">·</span>
            <span className="settings-item">
              <span className="settings-label">model</span>
              <span className="settings-value">{modelDisplay}</span>
            </span>
            <span className={`settings-caret ${showSettings ? 'open' : ''}`}>›</span>
          </button>

          <div className={`settings-panel desktop-only ${showSettings ? 'open' : ''}`}>
            <div className="settings-panel-inner">
              <SettingsFields {...settingsProps} />
            </div>
          </div>
          {bgToast && <div className="bg-toast">{bgToast}</div>}
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
