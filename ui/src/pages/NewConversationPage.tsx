import { useState, useEffect, useRef, KeyboardEvent, ClipboardEvent, ChangeEvent } from 'react';
import { useNavigate } from 'react-router-dom';
import { enhancedApi } from '../enhancedApi';
import { DirectoryPicker } from '../components/DirectoryPicker';
import { ImageAttachments } from '../components/ImageAttachments';
import { VoiceRecorder, isWebSpeechSupported } from '../components/VoiceInput';
import type { ModelsResponse, ImageData } from '../api';

const SUPPORTED_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];
const MAX_IMAGE_SIZE = 5 * 1024 * 1024; // 5MB

async function fileToBase64(file: File): Promise<ImageData> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result as string;
      const base64 = result.split(',')[1];
      resolve({
        data: base64,
        media_type: file.type,
      });
    };
    reader.onerror = () => reject(new Error('Failed to read file'));
    reader.readAsDataURL(file);
  });
}

export function NewConversationPage() {
  const navigate = useNavigate();
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  
  const [cwd, setCwd] = useState('/home/exedev');
  const [models, setModels] = useState<ModelsResponse | null>(null);
  const [selectedModel, setSelectedModel] = useState<string | null>(null);
  const [draft, setDraft] = useState('');
  const [images, setImages] = useState<ImageData[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [pathValid, setPathValid] = useState(true);
  
  // Voice input
  const voiceSupported = isWebSpeechSupported();
  const [interimText, setInterimText] = useState('');
  const draftBeforeVoiceRef = useRef<string>('');

  // Load models on mount
  useEffect(() => {
    enhancedApi.listModels().then(modelsData => {
      setModels(modelsData);
      setSelectedModel(modelsData.default);
    }).catch(err => {
      console.error('Failed to load models:', err);
      setError('Failed to load available models');
    });
  }, []);

  // Validate path
  useEffect(() => {
    const validate = async () => {
      const trimmed = cwd.trim();
      if (!trimmed) {
        setPathValid(false);
        return;
      }
      
      const validation = await enhancedApi.validateCwd(trimmed);
      if (validation.valid) {
        setPathValid(true);
        setError(null);
        return;
      }
      
      // Check if parent exists (we can create this directory)
      const parentPath = trimmed.substring(0, trimmed.lastIndexOf('/')) || '/';
      const parentValidation = await enhancedApi.validateCwd(parentPath);
      setPathValid(parentValidation.valid);
      setError(null);
    };
    
    validate();
  }, [cwd]);

  // Auto-resize textarea
  const autoResize = () => {
    const ta = textareaRef.current;
    if (ta) {
      ta.style.height = 'auto';
      ta.style.height = Math.min(ta.scrollHeight, 200) + 'px';
    }
  };

  useEffect(() => {
    autoResize();
  }, [draft]);

  // Focus textarea on mount
  useEffect(() => {
    textareaRef.current?.focus();
  }, []);

  const addImages = async (files: File[]) => {
    const validFiles = files.filter(file => {
      if (!SUPPORTED_TYPES.includes(file.type)) {
        console.warn(`Unsupported image type: ${file.type}`);
        return false;
      }
      if (file.size > MAX_IMAGE_SIZE) {
        console.warn(`Image too large: ${file.name}`);
        return false;
      }
      return true;
    });

    try {
      const newImages = await Promise.all(validFiles.map(fileToBase64));
      setImages([...images, ...newImages]);
    } catch (error) {
      console.error('Error processing images:', error);
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
    if (files.length > 0) {
      await addImages(files);
    }
    e.target.value = '';
  };

  const handleRemoveImage = (index: number) => {
    setImages(images.filter((_, i) => i !== index));
  };

  const handleSend = async () => {
    const trimmed = draft.trim();
    if (!trimmed && images.length === 0) return;
    if (!pathValid || creating) return;

    setError(null);
    setCreating(true);

    try {
      // Create directory if needed
      const validation = await enhancedApi.validateCwd(cwd.trim());
      if (!validation.valid) {
        const mkdirResult = await enhancedApi.mkdir(cwd.trim());
        if (!mkdirResult.created) {
          setError(mkdirResult.error || 'Failed to create directory');
          setCreating(false);
          return;
        }
      }

      // Generate message ID
      const messageId = crypto.randomUUID();

      // Create conversation with initial message
      const conv = await enhancedApi.createConversation(
        cwd.trim(),
        trimmed,
        messageId,
        selectedModel || undefined,
        images
      );

      // Navigate to the new conversation
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

  // Voice input handlers
  const handleVoiceFinal = (text: string) => {
    if (!text) return;
    setInterimText('');
    const baseDraft = draftBeforeVoiceRef.current || draft;
    const newDraft = baseDraft.trim() 
      ? baseDraft.trimEnd() + ' ' + text 
      : text;
    setDraft(newDraft);
    draftBeforeVoiceRef.current = newDraft;

    requestAnimationFrame(() => {
      if (textareaRef.current) {
        textareaRef.current.focus();
        const len = textareaRef.current.value.length;
        textareaRef.current.setSelectionRange(len, len);
      }
    });
  };

  const handleVoiceInterim = (text: string) => {
    if (!interimText && text) {
      draftBeforeVoiceRef.current = draft;
    }
    setInterimText(text);
  };

  const hasContent = draft.trim().length > 0 || images.length > 0;
  const canSend = hasContent && pathValid && !creating;

  return (
    <div id="app" className="new-conversation-page">
      <header className="new-conv-header">
        <button 
          className="back-btn" 
          onClick={() => navigate('/')}
          aria-label="Back to conversations"
        >
          ← Back
        </button>
        <h2>New Conversation</h2>
      </header>

      <main className="new-conv-main">
        <div className="new-conv-settings">
          <div className="setting-group">
            <label>Working Directory</label>
            <DirectoryPicker value={cwd} onChange={setCwd} />
          </div>

          <div className="setting-group">
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
          </div>
        </div>

        {error && (
          <div className="error-banner">
            {error}
          </div>
        )}

        <div className="new-conv-input-area">
          <ImageAttachments images={images} onRemove={handleRemoveImage} />
          
          <div className="input-row">
            <button
              className="attach-btn"
              onClick={() => fileInputRef.current?.click()}
              title="Attach image"
              aria-label="Attach image"
              disabled={creating}
            >
              📎
            </button>
            <input
              ref={fileInputRef}
              type="file"
              accept={SUPPORTED_TYPES.join(',')}
              multiple
              onChange={handleFileChange}
              style={{ display: 'none' }}
            />
            {voiceSupported && (
              <VoiceRecorder
                onSpeech={handleVoiceFinal}
                onInterim={handleVoiceInterim}
                disabled={creating}
              />
            )}
            <textarea
              ref={textareaRef}
              placeholder="What would you like to work on?"
              rows={3}
              value={interimText ? (draft.trim() ? draft.trimEnd() + ' ' + interimText : interimText) : draft}
              onChange={(e) => {
                setDraft(e.target.value);
                if (interimText) {
                  setInterimText('');
                  draftBeforeVoiceRef.current = '';
                }
              }}
              onKeyDown={handleKeyDown}
              onPaste={handlePaste}
              disabled={creating}
            />
            <button
              className="send-btn"
              onClick={handleSend}
              disabled={!canSend}
            >
              {creating ? 'Creating...' : 'Send'}
            </button>
          </div>
        </div>
      </main>
    </div>
  );
}
