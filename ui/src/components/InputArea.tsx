import { useRef, useEffect, KeyboardEvent, ClipboardEvent, ChangeEvent } from 'react';
import { useNavigate } from 'react-router-dom';
import type { QueuedMessage } from '../hooks';
import type { ImageData } from '../api';
import { ImageAttachments } from './ImageAttachments';

const SUPPORTED_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];
const MAX_IMAGE_SIZE = 5 * 1024 * 1024; // 5MB

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
  conversationSlug?: string;
  convState?: string;
}

async function fileToBase64(file: File): Promise<ImageData> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result as string;
      // Extract base64 data after the data URL prefix
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
  conversationSlug,
  convState,
}: InputAreaProps) {
  const navigate = useNavigate();
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const footerRef = useRef<HTMLElement>(null);

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

  // Handle mobile keyboard visual viewport changes
  useEffect(() => {
    const viewport = window.visualViewport;
    if (!viewport) return;

    const handleViewportChange = () => {
      const footer = footerRef.current;
      if (!footer) return;

      // Calculate the offset from the bottom of the layout viewport
      // to the bottom of the visual viewport (keyboard height)
      const offsetBottom = window.innerHeight - viewport.height - viewport.offsetTop;
      
      if (offsetBottom > 0) {
        // Keyboard is open - position input above it
        footer.style.bottom = `${offsetBottom}px`;
      } else {
        // Keyboard is closed - reset to default
        footer.style.bottom = '0px';
      }
    };

    viewport.addEventListener('resize', handleViewportChange);
    viewport.addEventListener('scroll', handleViewportChange);

    return () => {
      viewport.removeEventListener('resize', handleViewportChange);
      viewport.removeEventListener('scroll', handleViewportChange);
    };
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
    const trimmed = draft.trim();
    // Can send if there's text OR images
    if (!trimmed && images.length === 0) return;
    if (!canSend && !isOffline) return;
    onSend(trimmed, images);
    setImages([]); // Clear images after send
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const failedMessages = queuedMessages.filter(m => m.status === 'failed');
  const hasContent = draft.trim().length > 0 || images.length > 0;
  const sendEnabled = (canSend || isOffline) && hasContent;

  // Status indicator
  const getStatusIndicator = () => {
    if (isOffline) return { dot: 'offline', text: 'Offline' };
    if (agentWorking) return { dot: 'working', text: convState || 'Working...' };
    return { dot: 'idle', text: 'Ready' };
  };
  const status = getStatusIndicator();

  return (
    <footer id="input-area" ref={footerRef}>
      {/* Navigation and status row */}
      <div className="input-header">
        <button 
          className="back-btn" 
          onClick={() => navigate('/')}
          aria-label="Back to conversations"
        >
          ← Back
        </button>
        {conversationSlug && (
          <span className="conv-title">{conversationSlug}</span>
        )}
        <div className="status-compact">
          <span className={`dot ${status.dot}`} />
          <span className="status-text">{status.text}</span>
        </div>
      </div>

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
      
      <div id="input-container">
        <button
          className="attach-btn"
          onClick={() => fileInputRef.current?.click()}
          title="Attach image"
          aria-label="Attach image"
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
        <textarea
          ref={textareaRef}
          id="message-input"
          placeholder={isOffline ? 'Type a message (will send when back online)...' : 'Type a message...'}
          rows={1}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={handleKeyDown}
          onPaste={handlePaste}
        />
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
