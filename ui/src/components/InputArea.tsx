import { useRef, useEffect, useCallback, useState, KeyboardEvent, ClipboardEvent, ChangeEvent } from 'react';
import { FolderOpen } from 'lucide-react';
import type { QueuedMessage } from '../hooks';
import type { ImageData } from '../api';
import { ImageAttachments } from './ImageAttachments';
import { VoiceRecorder, isWebSpeechSupported } from './VoiceInput';
import { SUPPORTED_IMAGE_TYPES, processImageFiles } from '../utils/images';

interface InputAreaProps {
  draft: string;
  setDraft: (text: string) => void;
  images: ImageData[];
  setImages: (images: ImageData[]) => void;
  canSend: boolean;
  agentWorking: boolean;
  isCancelling: boolean;
  isOffline: boolean;
  queuedMessages: QueuedMessage[];
  onSend: (text: string, images: ImageData[]) => void;
  onCancel: () => void;
  onRetry: (localId: string) => void;
  onOpenFileBrowser?: () => void;
}



export function InputArea({
  draft,
  setDraft,
  images,
  setImages,
  canSend,
  agentWorking,
  isCancelling,
  isOffline,
  queuedMessages,
  onSend,
  onCancel,
  onRetry,
  onOpenFileBrowser,
}: InputAreaProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const voiceSupported = isWebSpeechSupported();
  
  // Voice input: base text (accumulated finals) + interim (current partial)
  const [voiceBase, setVoiceBase] = useState<string | null>(null); // null = not recording
  const [voiceInterim, setVoiceInterim] = useState('');


  const autoResize = () => {
    const ta = textareaRef.current;
    if (ta) {
      ta.style.height = 'auto';
      ta.style.height = Math.min(ta.scrollHeight, 120) + 'px';
    }
  };

  useEffect(() => {
    autoResize();
  }, [draft]);

  const addImages = async (files: File[]) => {
    try {
      const newImages = await processImageFiles(files);
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
      e.preventDefault(); // Prevent pasting image as text
      await addImages(imageFiles);
    }
  };

  const handleFileChange = async (e: ChangeEvent<HTMLInputElement>) => {
    const files = Array.from(e.target.files || []);
    if (files.length > 0) {
      await addImages(files);
    }
    // Reset input so same file can be selected again
    e.target.value = '';
  };

  const handleRemoveImage = (index: number) => {
    setImages(images.filter((_, i) => i !== index));
  };

  const handleSend = () => {
    // Get the current text (from voice recording or draft)
    let text: string;
    if (voiceBase !== null) {
      text = voiceBase.trim() + (voiceInterim ? ' ' + voiceInterim.trim() : '');
      // End recording state
      setVoiceBase(null);
      setVoiceInterim('');
      setDraft(''); // Clear draft too since we're sending
    } else {
      text = draft.trim();
      setDraft('');
    }
    
    // Can send if there's text OR images
    if (!text && images.length === 0) return;
    if (!canSend && !isOffline) return;
    onSend(text, images);
    setImages([]); // Clear images after send
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  // Voice recording started - capture current draft as base
  const handleVoiceStart = useCallback(() => {
    setVoiceBase(draft);
    setVoiceInterim('');
  }, [draft]);

  // Voice recording ended - sync final state to draft
  const handleVoiceEnd = useCallback(() => {
    setVoiceBase(prev => {
      if (prev !== null) {
        setDraft(prev);
      }
      return null;
    });
    setVoiceInterim('');
  }, [setDraft]);

  // Final transcript - append to base permanently
  const handleVoiceFinal = useCallback((text: string) => {
    if (!text) return;
    setVoiceBase(prev => {
      if (prev === null) return null;
      return prev.trim() ? prev.trimEnd() + ' ' + text : text;
    });
    setVoiceInterim(''); // Clear interim, final is now in base
  }, []);

  // Interim transcript - show temporarily (will be replaced)
  const handleVoiceInterim = useCallback((text: string) => {
    setVoiceInterim(text);
  }, []);

  const failedMessages = queuedMessages.filter(m => m.status === 'failed');
  const displayedText = voiceBase !== null ? voiceBase : draft;
  const hasContent = displayedText.trim().length > 0 || voiceInterim.trim().length > 0 || images.length > 0;
  const sendEnabled = (canSend || isOffline) && hasContent;

  return (
    <footer id="input-area">
      {failedMessages.length > 0 && (
        <div className="failed-messages">
          {failedMessages.map(msg => (
            <div key={msg.localId} className="failed-message">
              <span className="failed-message-icon">⚠️</span>
              <span className="failed-message-text">
                Failed to send: "{msg.text.length > 50 ? msg.text.slice(0, 50) + '...' : msg.text}"
              </span>
              <button
                className="failed-message-retry"
                onClick={() => onRetry(msg.localId)}
              >
                Retry
              </button>
            </div>
          ))}
        </div>
      )}
      
      <ImageAttachments images={images} onRemove={handleRemoveImage} />
      
      {/* Hidden file input */}
      <input
        ref={fileInputRef}
        type="file"
        accept={SUPPORTED_IMAGE_TYPES.join(',')}
        multiple
        onChange={handleFileChange}
        style={{ display: 'none' }}
      />
      
      {/* Full-width textarea */}
      <textarea
        ref={textareaRef}
        id="message-input"
        placeholder={isOffline ? 'Type a message (will send when back online)...' : 'Type a message...'}
        rows={2}
        value={voiceBase !== null
          ? (voiceBase.trim()
              ? voiceBase.trimEnd() + (voiceInterim ? ' ' + voiceInterim : '')
              : voiceInterim)
          : draft}
        onChange={(e) => {
          // If recording, update voice base; otherwise update draft
          if (voiceBase !== null) {
            setVoiceBase(e.target.value);
            setVoiceInterim('');
          } else {
            setDraft(e.target.value);
          }
        }}
        onKeyDown={handleKeyDown}
        onPaste={handlePaste}
      />
      
      {/* Action row: icons left, send/cancel right */}
      <div id="input-actions">
        <div className="input-actions-left">
          {onOpenFileBrowser && (
            <button
              className="file-browse-btn"
              onClick={onOpenFileBrowser}
              title="Browse files"
              aria-label="Browse files"
            >
              <FolderOpen size={20} />
            </button>
          )}
          <button
            className="attach-btn"
            onClick={() => fileInputRef.current?.click()}
            title="Attach image"
            aria-label="Attach image"
          >
            📎
          </button>
          {voiceSupported && (
            <VoiceRecorder
              onStart={handleVoiceStart}
              onEnd={handleVoiceEnd}
              onSpeech={handleVoiceFinal}
              onInterim={handleVoiceInterim}
              disabled={agentWorking}
            />
          )}
        </div>
        {agentWorking ? (
          <button
            id="cancel-btn"
            onClick={onCancel}
            disabled={isCancelling || isOffline}
          >
            {isCancelling ? 'Cancelling...' : 'Cancel'}
          </button>
        ) : (
          <button
            id="send-btn"
            onClick={handleSend}
            disabled={!sendEnabled}
          >
            Send
          </button>
        )}
      </div>
    </footer>
  );
}
