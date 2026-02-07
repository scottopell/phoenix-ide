import { useState, useEffect, useRef, KeyboardEvent, ClipboardEvent, ChangeEvent } from 'react';
import { useNavigate } from 'react-router-dom';
import { enhancedApi } from '../enhancedApi';
import { ImageAttachments } from '../components/ImageAttachments';
import { VoiceRecorder, isWebSpeechSupported } from '../components/VoiceInput';
import type { ModelsResponse, ImageData } from '../api';

const SUPPORTED_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];
const MAX_IMAGE_SIZE = 5 * 1024 * 1024;
const LAST_CWD_KEY = 'phoenix-last-cwd';
const LAST_MODEL_KEY = 'phoenix-last-model';

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
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const cwdInputRef = useRef<HTMLInputElement>(null);
  
  const [cwd, setCwd] = useState(() => localStorage.getItem(LAST_CWD_KEY) || '/home/exedev');
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

  useEffect(() => {
    enhancedApi.listModels().then(modelsData => {
      setModels(modelsData);
      if (!selectedModel) setSelectedModel(modelsData.default);
    }).catch(console.error);
  }, [selectedModel]);

  useEffect(() => { localStorage.setItem(LAST_CWD_KEY, cwd); }, [cwd]);
  useEffect(() => { if (selectedModel) localStorage.setItem(LAST_MODEL_KEY, selectedModel); }, [selectedModel]);

  const autoResize = () => {
    const ta = textareaRef.current;
    if (ta) {
      ta.style.height = 'auto';
      ta.style.height = Math.min(ta.scrollHeight, 200) + 'px';
    }
  };

  useEffect(() => { autoResize(); }, [draft]);
  useEffect(() => { textareaRef.current?.focus(); }, []);

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
    if (creating) return;

    setError(null);
    setCreating(true);

    try {
      const validation = await enhancedApi.validateCwd(cwd.trim());
      if (!validation.valid) {
        const mkdirResult = await enhancedApi.mkdir(cwd.trim());
        if (!mkdirResult.created) {
          setError(mkdirResult.error || 'Invalid directory');
          setCreating(false);
          return;
        }
      }

      const messageId = crypto.randomUUID();
      const conv = await enhancedApi.createConversation(
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
  const canSend = hasContent && !creating;

  const cwdDisplay = cwd.replace(/^\/home\/exedev\/?/, '~/') || '~/';
  const modelDisplay = models?.models.find(m => m.id === selectedModel)?.id.replace('-sonnet', '').replace('-opus', '') || 'loading...';

  return (
    <div className="new-conv-page">
      <header className="new-conv-header-minimal">
        <button className="back-link" onClick={() => navigate('/')}>← Back</button>
      </header>

      <main className="new-conv-center">
        <h1 className="new-conv-title">New conversation</h1>
        
        {error && <div className="new-conv-error">{error}</div>}

        <div className="new-conv-input-box">
          <ImageAttachments images={images} onRemove={(i) => setImages(images.filter((_, idx) => idx !== i))} />
          
          <textarea
            ref={textareaRef}
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
              <input ref={fileInputRef} type="file" accept={SUPPORTED_TYPES.join(',')} multiple onChange={handleFileChange} style={{ display: 'none' }} />
              {voiceSupported && <VoiceRecorder onSpeech={handleVoiceFinal} onInterim={handleVoiceInterim} disabled={creating} />}
            </div>
            <button className="new-conv-send" onClick={handleSend} disabled={!canSend}>
              {creating ? 'Creating...' : 'Send'}
            </button>
          </div>
        </div>

        {/* Settings row */}
        <button className="settings-row" onClick={() => setShowSettings(!showSettings)}>
          <span className="settings-item">
            <span className="settings-label">Directory</span>
            <span className="settings-value">{cwdDisplay}</span>
          </span>
          <span className="settings-dot">·</span>
          <span className="settings-item">
            <span className="settings-label">Model</span>
            <span className="settings-value">{modelDisplay}</span>
          </span>
          <span className={`settings-caret ${showSettings ? 'open' : ''}`}>›</span>
        </button>

        {/* Expandable settings */}
        <div className={`settings-panel ${showSettings ? 'open' : ''}`}>
          <div className="settings-panel-inner">
            <label className="settings-field">
              <span className="settings-field-label">Working Directory</span>
              <input
                ref={cwdInputRef}
                type="text"
                className="settings-input"
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
      </main>
    </div>
  );
}
