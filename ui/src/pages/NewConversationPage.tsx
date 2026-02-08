import { useState, useEffect, useRef, KeyboardEvent, ClipboardEvent, ChangeEvent } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api';
import { ImageAttachments } from '../components/ImageAttachments';
import { VoiceRecorder, isWebSpeechSupported } from '../components/VoiceInput';
import type { ModelsResponse, ImageData } from '../api';

const SUPPORTED_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];
const MAX_IMAGE_SIZE = 5 * 1024 * 1024;
const LAST_CWD_KEY = 'phoenix-last-cwd';
const LAST_MODEL_KEY = 'phoenix-last-model';

type DirStatus = 'checking' | 'exists' | 'will-create' | 'invalid';

async function fileToBase64(file: File): Promise<ImageData> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result as string;
      const base64 = result.split(',')[1];
      resolve({ data: base64, media_type: file.type });
    };
    reader.onerror = () => reject(new Error('Failed to read file'));
    reader.readAsDataURL(file);
  });
}

export function NewConversationPage() {
  const navigate = useNavigate();
  const textareaDesktopRef = useRef<HTMLTextAreaElement>(null);
  const textareaMobileRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  
  const [cwd, setCwd] = useState(() => localStorage.getItem(LAST_CWD_KEY) || '/home/exedev');
  const [dirStatus, setDirStatus] = useState<DirStatus>('checking');
  const [models, setModels] = useState<ModelsResponse | null>(null);
  const [selectedModel, setSelectedModel] = useState<string | null>(() => localStorage.getItem(LAST_MODEL_KEY));
  const [draft, setDraft] = useState('');
  const [images, setImages] = useState<ImageData[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  
  const voiceSupported = isWebSpeechSupported();
  const [interimText, setInterimText] = useState('');
  const draftBeforeVoiceRef = useRef<string>('');

  // Load models
  useEffect(() => {
    api.listModels().then(modelsData => {
      setModels(modelsData);
      if (!selectedModel) setSelectedModel(modelsData.default);
    }).catch(console.error);
  }, [selectedModel]);

  // Validate directory path
  useEffect(() => {
    const trimmed = cwd.trim();
    if (!trimmed || !trimmed.startsWith('/')) {
      setDirStatus('invalid');
      return;
    }

    setDirStatus('checking');
    const timeoutId = setTimeout(async () => {
      try {
        const validation = await api.validateCwd(trimmed);
        if (validation.valid) {
          setDirStatus('exists');
        } else {
          // Check if parent exists (can create)
          const parentPath = trimmed.substring(0, trimmed.lastIndexOf('/')) || '/';
          const parentValidation = await api.validateCwd(parentPath);
          setDirStatus(parentValidation.valid ? 'will-create' : 'invalid');
        }
      } catch {
        setDirStatus('invalid');
      }
    }, 300); // Debounce

    return () => clearTimeout(timeoutId);
  }, [cwd]);

  // Save preferences
  useEffect(() => { localStorage.setItem(LAST_CWD_KEY, cwd); }, [cwd]);
  useEffect(() => { if (selectedModel) localStorage.setItem(LAST_MODEL_KEY, selectedModel); }, [selectedModel]);

  // Auto-resize textareas
  useEffect(() => {
    [textareaDesktopRef.current, textareaMobileRef.current].forEach(ta => {
      if (ta) {
        ta.style.height = 'auto';
        ta.style.height = Math.min(ta.scrollHeight, 200) + 'px';
      }
    });
  }, [draft]);

  // Focus appropriate textarea on mount (mobile vs desktop)
  useEffect(() => {
    const isMobile = window.innerWidth <= 768;
    if (isMobile) {
      textareaMobileRef.current?.focus();
    } else {
      textareaDesktopRef.current?.focus();
    }
  }, []);

  const addImages = async (files: File[]) => {
    const validFiles = files.filter(f => SUPPORTED_TYPES.includes(f.type) && f.size <= MAX_IMAGE_SIZE);
    try {
      const newImages = await Promise.all(validFiles.map(fileToBase64));
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

  const handleSend = async () => {
    const trimmed = draft.trim();
    if (!trimmed && images.length === 0) return;
    if (creating || dirStatus === 'invalid' || dirStatus === 'checking') return;

    setError(null);
    setCreating(true);

    try {
      // Create directory if needed
      if (dirStatus === 'will-create') {
        const mkdirResult = await api.mkdir(cwd.trim());
        if (!mkdirResult.created) {
          setError(mkdirResult.error || 'Failed to create directory');
          setCreating(false);
          return;
        }
      }

      const messageId = crypto.randomUUID();
      const conv = await api.createConversation(
        cwd.trim(), trimmed, messageId, selectedModel || undefined, images
      );
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

  const hasContent = draft.trim().length > 0 || images.length > 0;
  const canSend = hasContent && !creating && dirStatus !== 'invalid' && dirStatus !== 'checking';

  // Display values
  const cwdDisplay = cwd.trim().replace(/^\/home\/exedev\/?/, '~/') || '~/';
  const modelDisplay = models?.models.find(m => m.id === selectedModel)?.id.replace('-sonnet', '').replace('-opus', '') || '...';

  // Status indicator for directory
  const dirStatusIcon = {
    'checking': '...',
    'exists': '✓',
    'will-create': '+',
    'invalid': '✗',
  }[dirStatus];

  const dirStatusClass = {
    'checking': 'status-checking',
    'exists': 'status-ok',
    'will-create': 'status-create',
    'invalid': 'status-error',
  }[dirStatus];

  // Button text
  const buttonText = creating 
    ? (dirStatus === 'will-create' ? 'Creating folder...' : 'Creating...')
    : 'Send';

  return (
    <div className="new-conv-page">
      {/* Hidden file input - shared by both desktop and mobile */}
      <input ref={fileInputRef} type="file" accept={SUPPORTED_TYPES.join(',')} multiple onChange={handleFileChange} style={{ display: 'none' }} />
      
      <header className="new-conv-header-minimal">
        <button className="back-link" onClick={() => navigate('/')}>← Back</button>
      </header>

      {/* Main content area - settings visible on mobile */}
      <main className="new-conv-main">
        <div className="new-conv-content">
          <h1 className="new-conv-title">New conversation</h1>
          
          {error && <div className="new-conv-error">{error}</div>}

          {/* Settings card - always visible on mobile, collapsible on desktop */}
          <div className="new-conv-settings-card">
            <label className="settings-field">
              <span className="settings-field-label">
                Directory
                <span className={`field-status ${dirStatusClass}`}>
                  {dirStatus === 'exists' && 'exists'}
                  {dirStatus === 'will-create' && 'will be created'}
                  {dirStatus === 'invalid' && 'invalid path'}
                  {dirStatus === 'checking' && 'checking...'}
                </span>
              </span>
              <input
                type="text"
                className={`settings-input ${dirStatusClass}`}
                value={cwd}
                onChange={(e) => setCwd(e.target.value)}
                placeholder="/path/to/project"
              />
            </label>
            <label className="settings-field">
              <span className="settings-field-label">Model</span>
              <select
                className="settings-select"
                value={selectedModel || ''}
                onChange={(e) => setSelectedModel(e.target.value)}
                disabled={!models}
              >
                {models?.models.map(m => (
                  <option key={m.id} value={m.id}>{m.id}</option>
                ))}
              </select>
            </label>
          </div>

          {/* Desktop: centered input box */}
          <div className="new-conv-input-box desktop-only">
            <ImageAttachments images={images} onRemove={(i) => setImages(images.filter((_, idx) => idx !== i))} />
            
            <textarea
              ref={textareaDesktopRef}
              className="new-conv-textarea"
              placeholder="What would you like to work on?"
              rows={3}
              value={interimText ? (draft.trim() ? draft.trimEnd() + ' ' + interimText : interimText) : draft}
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
              <button className="new-conv-send" onClick={handleSend} disabled={!canSend}>
                {buttonText}
              </button>
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
              <label className="settings-field">
                <span className="settings-field-label">
                  Directory
                  <span className={`field-status ${dirStatusClass}`}>
                    {dirStatus === 'exists' && 'exists'}
                    {dirStatus === 'will-create' && 'will be created'}
                    {dirStatus === 'invalid' && 'invalid path'}
                    {dirStatus === 'checking' && 'checking...'}
                  </span>
                </span>
                <input
                  type="text"
                  className={`settings-input ${dirStatusClass}`}
                  value={cwd}
                  onChange={(e) => setCwd(e.target.value)}
                  placeholder="/path/to/project"
                />
              </label>
              <label className="settings-field">
                <span className="settings-field-label">Model</span>
                <select
                  className="settings-select"
                  value={selectedModel || ''}
                  onChange={(e) => setSelectedModel(e.target.value)}
                  disabled={!models}
                >
                  {models?.models.map(m => (
                    <option key={m.id} value={m.id}>{m.id}</option>
                  ))}
                </select>
              </label>
            </div>
          </div>
        </div>
      </main>

      {/* Mobile: bottom-anchored input */}
      <div className="new-conv-bottom-input mobile-only">
        <ImageAttachments images={images} onRemove={(i) => setImages(images.filter((_, idx) => idx !== i))} />
        <div className="new-conv-input-row">
          <div className="new-conv-input-left">
            <button className="icon-btn" onClick={() => fileInputRef.current?.click()} title="Attach image" disabled={creating}>📎</button>
            {voiceSupported && <VoiceRecorder onSpeech={handleVoiceFinal} onInterim={handleVoiceInterim} disabled={creating} />}
          </div>
          <textarea
            ref={textareaMobileRef}
            className="new-conv-textarea-mobile"
            placeholder="What would you like to work on?"
            rows={2}
            value={interimText ? (draft.trim() ? draft.trimEnd() + ' ' + interimText : interimText) : draft}
            onChange={(e) => {
              setDraft(e.target.value);
              if (interimText) { setInterimText(''); draftBeforeVoiceRef.current = ''; }
            }}
            onKeyDown={handleKeyDown}
            onPaste={handlePaste}
            disabled={creating}
          />
          <button className="new-conv-send" onClick={handleSend} disabled={!canSend}>
            {buttonText}
          </button>
        </div>
      </div>
    </div>
  );
}
