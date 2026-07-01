import {
  useRef,
  useEffect,
  useCallback,
  useState,
  forwardRef,
  useImperativeHandle,
  KeyboardEvent,
  ClipboardEvent,
  ChangeEvent,
  DragEvent,
  type SetStateAction,
} from 'react';
// Icon buttons removed from action row -- file browse via sidebar, image attach via paste/drag
import type { QueuedMessage } from '../hooks';
import { useDraftActions, useDraftValue, useScopedState, useInlineReferences } from '../hooks';
import type { ConversationState, FileAttachment, ImageData } from '../api';
import { api, ExpansionError, MAX_FILE_ATTACHMENT_SIZE, MAX_FILE_ATTACHMENTS, MAX_TOTAL_FILE_ATTACHMENT_SIZE } from '../api';
import { canCancelConversationState, isAgentWorking, isCancellingState } from '../utils';
import { ImageAttachments } from './ImageAttachments';
import { VoiceRecorder, isWebSpeechSupported } from './VoiceInput';
import { SUPPORTED_IMAGE_TYPES, processImageFiles } from '../utils/images';
import { FILE_TREE_DRAG_TYPE } from './FileExplorer/FileTree';

export interface InputAreaHandle {
  focus: () => void;
}

interface InputAreaProps {
  /**
   * Working directory the conversation operates in. Scopes inline reference
   * autocomplete (`@file`, `./path`, `/skill`) to the same root that
   * `message_expander::expand` resolves against at send time. `undefined`
   * disables autocomplete fetching.
   */
  cwd: string | undefined;
  /**
   * Composer identity (the conversation id). Transient composer state — voice
   * buffer, active autocomplete trigger, inline expansion error — resets when
   * this changes, so switching between two conversations that share a `cwd`
   * doesn't carry one's state into the other.
   */
  scopeKey: string | undefined;
  convState: ConversationState;
  images: ImageData[];
  setImages: (images: ImageData[]) => void;
  files?: FileAttachment[];
  setFiles?: (files: SetStateAction<FileAttachment[]>) => void;
  isOffline: boolean;
  /**
   * Messages whose POST was rejected. Rendered inline with retry/dismiss
   * controls. The caller pre-filters — InputArea does not know about the
   * full queue, only the failures surfacing here.
   */
  failedMessages: QueuedMessage[];
  /** Conversation mode label (e.g. "Explore", "Work", "Direct") */
  convModeLabel?: string | undefined;
  /** Controlled draft text. This component is purely presentational —
   *  the source of truth lives in `DraftStore`, fed in by
   *  `<ConnectedInputArea>`. */
  draft: string;
  /** Called for every draft mutation (keystroke, autocomplete apply, voice
   *  commit, paste). */
  onDraftChange: (text: string) => void;
  /**
   * Monotonic counter bumped by the parent when it wants the textarea
   * focused (terminal selection arrival, retry-of-failed, skill insert,
   * prose-reader send-notes). Survives unmount/remount: a parent bump while
   * InputArea is unmounted lands as a fresh effect tick on re-mount.
   */
  focusToken?: number;
  /**
   * Called when the user sends a message.
   * May reject with an expansion error (REQ-IR-007) — the component will
   * display the error inline without clearing the draft.
   */
  onSend: (text: string, images: ImageData[], files: FileAttachment[]) => Promise<void> | void;
  onCancel: () => void;
  onRetry: (localId: string) => void;
  onDismissError?: (localId: string) => void;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export const InputArea = forwardRef<InputAreaHandle, InputAreaProps>(function InputArea({
  cwd,
  scopeKey,
  convState,
  images,
  setImages,
  files = [],
  setFiles = () => {},
  isOffline,
  failedMessages,
  convModeLabel,
  draft,
  onDraftChange,
  focusToken,
  onSend,
  onCancel,
  onRetry,
  onDismissError,
}, ref) {
  const agentWorking = isAgentWorking(convState);
  const canCancel = canCancelConversationState(convState);
  const isCancelling = isCancellingState(convState);
  const blocksComposerSend =
    isCancelling ||
    convState.type === 'awaiting_llm' ||
    convState.type === 'awaiting_continuation';
  const setDraft = onDraftChange;
  const clearDraft = useCallback(() => onDraftChange(''), [onDraftChange]);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  // `scopeKey` is the conversation id for this (always in-conversation) composer;
  // attachment uploads target it, and this ref lets an in-flight upload detect a
  // conversation switch and discard its late response.
  const scopeKeyRef = useRef(scopeKey);
  useEffect(() => { scopeKeyRef.current = scopeKey; }, [scopeKey]);
  const voiceSupported = isWebSpeechSupported();

  useImperativeHandle(ref, () => ({
    focus: () => {
      textareaRef.current?.focus();
    },
  }), []);

  // Consume parent's focus-token bumps. Skipping the initial 0 means
  // mounting alone doesn't steal focus — the parent has to explicitly ask.
  useEffect(() => {
    if (!focusToken) return;
    textareaRef.current?.focus();
  }, [focusToken]);

  // Voice input: base text (accumulated finals) + interim (current partial)
  const [voiceBase, setVoiceBase] = useScopedState<string | null>(scopeKey, null); // null = not recording
  const [voiceInterim, setVoiceInterim] = useScopedState(scopeKey, '');

  // =========================================================================
  // Inline autocomplete (REQ-IR-004, REQ-IR-005), scoped to `cwd`
  // =========================================================================

  // The autocomplete operates on whichever text is live: the in-progress voice
  // transcript while recording, otherwise the draft.
  const acValue = voiceBase !== null ? voiceBase : draft;
  const applyRefValue = useCallback(
    (next: string) => {
      if (voiceBase !== null) {
        setVoiceBase(next);
      } else {
        setDraft(next);
      }
    },
    [voiceBase, setVoiceBase, setDraft],
  );

  const ir = useInlineReferences({
    cwd,
    scopeKey,
    textareaRef,
    value: acValue,
    setValue: applyRefValue,
  });
  // Inline expansion-error surface is shared between @ref/skill expansion and
  // attachment-upload failures, so the file-drop handlers reuse it.
  const { reset: resetRefs, setExpansionError, expansionError } = ir;

  // File-attachment drag/drop state.
  const [isDragOver, setIsDragOver] = useState(false);
  const [isUploadingFiles, setIsUploadingFiles] = useState(false);

  // External draft insertions (file-tree context menu, drag-and-drop) bypass
  // handleChange/refValueChange, so expansionError is never cleared. Listen
  // for the insert event and clear the error so Send isn't left disabled.
  useEffect(() => {
    const handler = () => setExpansionError(null);
    window.addEventListener('phoenix:insert-draft', handler);
    return () => window.removeEventListener('phoenix:insert-draft', handler);
  }, [setExpansionError]);

  // =========================================================================
  // Auto-resize
  // =========================================================================

  const autoResize = useCallback(() => {
    const ta = textareaRef.current;
    if (ta) {
      ta.style.height = 'auto';
      ta.style.height = Math.min(ta.scrollHeight, 120) + 'px';
    }
  }, []);

  useEffect(() => {
    autoResize();
  }, [draft, autoResize]);

  // =========================================================================
  // Image handling
  // =========================================================================

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

  const handleRemoveFile = (index: number) => {
    setFiles(files.filter((_, i) => i !== index));
  };

  const addDroppedFiles = async (dropped: File[]) => {
    if (isUploadingFiles) {
      setExpansionError('Wait for the current attachment upload to finish before adding more files.');
      return;
    }
    if (!scopeKey) {
      setExpansionError('Open a conversation before attaching files.');
      return;
    }

    const uploadConversationId = scopeKey;

    const unsupportedImage = dropped.find(file => file.type.startsWith('image/') && !SUPPORTED_IMAGE_TYPES.includes(file.type));
    if (unsupportedImage) {
      setExpansionError(`${unsupportedImage.name} is not a supported image attachment type.`);
      return;
    }
    const imageFiles = dropped.filter(file => SUPPORTED_IMAGE_TYPES.includes(file.type));
    const genericFiles = dropped.filter(file => !SUPPORTED_IMAGE_TYPES.includes(file.type));

    const tooLarge = genericFiles.find(file => file.size > MAX_FILE_ATTACHMENT_SIZE);
    if (tooLarge) {
      setExpansionError(`${tooLarge.name} exceeds the 10 MB file attachment limit.`);
      return;
    }
    if (files.length + genericFiles.length > MAX_FILE_ATTACHMENTS) {
      setExpansionError(`A message can include at most ${MAX_FILE_ATTACHMENTS} files.`);
      return;
    }
    const total = files.reduce((sum, file) => sum + file.size_bytes, 0)
      + genericFiles.reduce((sum, file) => sum + file.size, 0);
    if (total > MAX_TOTAL_FILE_ATTACHMENT_SIZE) {
      setExpansionError('Attachments exceed the 25 MB total limit.');
      return;
    }

    if (imageFiles.length > 0) await addImages(imageFiles);
    if (genericFiles.length === 0) return;

    setIsUploadingFiles(true);
    setExpansionError(null);
    try {
      const uploaded = await api.uploadAttachments(uploadConversationId, genericFiles);
      if (scopeKeyRef.current !== uploadConversationId) return;
      setFiles(prev => [...prev, ...uploaded]);
    } catch (err) {
      setExpansionError(err instanceof Error ? err.message : 'Failed to upload attachments');
    } finally {
      setIsUploadingFiles(false);
    }
  };

  const handleDragEnter = (e: DragEvent) => {
    const types = Array.from(e.dataTransfer.types);
    if (types.includes(FILE_TREE_DRAG_TYPE) || types.includes('Files')) {
      e.preventDefault();
      setIsDragOver(true);
    }
  };

  const handleDragOver = (e: DragEvent) => {
    const types = Array.from(e.dataTransfer.types);
    if (types.includes(FILE_TREE_DRAG_TYPE) || types.includes('Files')) {
      e.preventDefault();
      e.dataTransfer.dropEffect = 'copy';
    }
  };

  const handleDragLeave = (e: DragEvent) => {
    if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
      setIsDragOver(false);
    }
  };

  const handleDrop = async (e: DragEvent) => {
    const types = Array.from(e.dataTransfer.types);

    // File-tree → composer drag: insert an @file reference for files (include
    // contents at send time) or a ./path reference for directories (point the
    // AI at the path without expansion, since @ expansion rejects trailing /).
    // Checked before the OS Files path so the two drop modes don't conflict.
    if (types.includes(FILE_TREE_DRAG_TYPE)) {
      e.preventDefault();
      setIsDragOver(false);
      const raw = e.dataTransfer.getData(FILE_TREE_DRAG_TYPE);
      if (raw) {
        try {
          const payload = JSON.parse(raw) as { relativePath: string; isDirectory: boolean; isText: boolean };
          // @ expansion is text-only — directories and non-text files (images,
          // binaries) use ./path so the AI gets a pointer without a blocked
          // expansion attempt at send time.
          const useAtRef = !payload.isDirectory && payload.isText;
          const ref = useAtRef
            ? `@${payload.relativePath} `
            : `./${payload.relativePath} `;
          window.dispatchEvent(new CustomEvent('phoenix:insert-draft', { detail: { text: ref } }));
        } catch {
          // Malformed drag payload — silently ignore
        }
      }
      return;
    }

    if (!types.includes('Files')) return;
    e.preventDefault();
    setIsDragOver(false);
    if (isUploadingFiles) {
      setExpansionError('Wait for the current attachment upload to finish before adding more files.');
      return;
    }
    const dropped = Array.from(e.dataTransfer.files);
    if (dropped.length > 0) await addDroppedFiles(dropped);
  };

  // =========================================================================
  // Send (with expansion error handling — REQ-IR-007)
  // =========================================================================

  const handleSend = useCallback(async () => {
    let text: string;
    if (voiceBase !== null) {
      text = voiceBase.trim() + (voiceInterim ? ' ' + voiceInterim.trim() : '');
    } else {
      text = draft.trim();
    }

    if (!text && images.length === 0 && files.length === 0) return;
    if (blocksComposerSend || isUploadingFiles) return;
    // The Send button is disabled while an expansion error is shown; gate the
    // keyboard (Enter) path too so a stale broken @reference isn't resubmitted
    // before it's corrected (REQ-IR-007).
    if (expansionError) return;
    // Allow send while agent is working — the server will queue it as a
    // steering message and deliver it when the conversation reaches Idle.

    // Close autocomplete and ghost text on send
    resetRefs();

    // Clear draft and attachments eagerly — the message is already queued and
    // visible in the message list, so there's no reason to keep the text in
    // the input area while waiting for the network round-trip.  We only
    // restore the draft if an ExpansionError comes back (user must fix the
    // broken @reference before re-sending).
    const previousVoiceBase = voiceBase;
    if (voiceBase !== null) {
      setVoiceBase(null);
      setVoiceInterim('');
    }
    clearDraft();
    setImages([]);
    setFiles([]);
    setExpansionError(null);

    try {
      await onSend(text, images, files);
    } catch (err) {
      if (err instanceof ExpansionError) {
        // Surface expansion error inline and restore the draft (REQ-IR-007)
        // so the user can fix or remove the broken @reference.
        setExpansionError(err.detail.error);
        if (previousVoiceBase !== null) {
          setVoiceBase(previousVoiceBase);
        } else {
          setDraft(text);
        }
        setImages(images);
        setFiles(files);
      }
      // Non-expansion errors: draft is already cleared; the message queue
      // shows the failure with a retry button.
    }
  }, [
    voiceBase,
    voiceInterim,
    draft,
    images,
    files,
    blocksComposerSend,
    expansionError,
    isUploadingFiles,
    onSend,
    clearDraft,
    resetRefs,
    setExpansionError,
    setImages,
    setFiles,
    setVoiceBase,
    setVoiceInterim,
    setDraft,
  ]);

  // =========================================================================
  // Keyboard handling (autocomplete nav first, then send)
  // =========================================================================

  const { onKeyDown: refKeyDown } = ir;

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLTextAreaElement>) => {
      if (refKeyDown(e)) return;
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        handleSend();
      }
    },
    [refKeyDown, handleSend],
  );

  // =========================================================================
  // Voice input
  // =========================================================================

  const handleVoiceStart = useCallback(() => {
    setVoiceBase(draft);
    setVoiceInterim('');
  }, [draft, setVoiceBase, setVoiceInterim]);

  const handleVoiceEnd = useCallback(() => {
    setVoiceBase(prev => {
      if (prev !== null) {
        setDraft(prev);
      }
      return null;
    });
    setVoiceInterim('');
  }, [setDraft, setVoiceBase, setVoiceInterim]);

  const handleVoiceFinal = useCallback((text: string) => {
    if (!text) return;
    setVoiceBase(prev => {
      if (prev === null) return null;
      return prev.trim() ? prev.trimEnd() + ' ' + text : text;
    });
    setVoiceInterim('');
  }, [setVoiceBase, setVoiceInterim]);

  const handleVoiceInterim = useCallback((text: string) => {
    setVoiceInterim(text);
  }, [setVoiceInterim]);

  // =========================================================================
  // Textarea event handlers
  // =========================================================================

  const { onValueChange: refValueChange } = ir;

  const handleChange = useCallback((e: ChangeEvent<HTMLTextAreaElement>) => {
    const newVal = e.target.value;
    if (voiceBase !== null) {
      setVoiceBase(newVal);
      setVoiceInterim('');
    } else {
      setDraft(newVal);
    }
    refValueChange(newVal);
  }, [voiceBase, setVoiceBase, setVoiceInterim, setDraft, refValueChange]);

  // =========================================================================
  // Derived state
  // =========================================================================

  const displayedText = voiceBase !== null ? voiceBase : draft;
  const hasContent = displayedText.trim().length > 0 || voiceInterim.trim().length > 0 || images.length > 0 || files.length > 0;
  const canSend = !blocksComposerSend && !isUploadingFiles;
  const sendEnabled = canSend && hasContent && !expansionError;

  // Cycle placeholder hint each time the input clears (e.g., after send).
  // Advances only when draft goes empty, not on a timer.
  const placeholderHints = ['', '/ for skills', '@ to include files', '? for shortcuts'];
  const [hintIndex, setHintIndex] = useState(0);
  const prevDraftRef = useRef(draft);

  useEffect(() => {
    const wasFilled = prevDraftRef.current.trim().length > 0;
    const nowEmpty = draft.trim().length === 0;
    if (wasFilled && nowEmpty) {
      setHintIndex(i => (i + 1) % placeholderHints.length);
    }
    prevDraftRef.current = draft;
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft]);

  const hint = placeholderHints[hintIndex];
  const isExplore = convModeLabel?.toLowerCase() === 'explore';
  const baseText = isExplore
    ? 'Explore this codebase or describe a change to plan...'
    : 'Type a message...';
  const placeholder = isOffline
    ? 'Type a message (will send when back online)...'
    : convState.type === 'awaiting_continuation'
      ? 'Compacting conversation...'
      : isCancelling
        ? 'Stopping...'
        : convState.type === 'awaiting_llm'
          ? 'Preparing request...'
          : agentWorking
            ? 'Agent working... send to queue a follow-up'
            : hint ? `${baseText} (${hint})` : baseText;

  // =========================================================================
  // Render
  // =========================================================================

  return (
    <footer
      id="input-area"
      className={isDragOver ? 'input-area--drag-over' : undefined}
      onDragEnter={handleDragEnter}
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
    >
      {failedMessages.length > 0 && (
        <div className="failed-messages">
          {failedMessages.map(msg => (
            <div key={msg.localId} className="failed-message">
              <span className="failed-message-icon">!</span>
              <span className="failed-message-text">
                Failed to send: "{msg.text.length > 50 ? msg.text.slice(0, 50) + '...' : msg.text}"
              </span>
              <button
                className="failed-message-retry"
                onClick={() => onRetry(msg.localId)}
              >
                Retry
              </button>
              {onDismissError && (
                <button
                  className="failed-message-dismiss"
                  onClick={() => onDismissError(msg.localId)}
                  title="Dismiss"
                >
                  x
                </button>
              )}
            </div>
          ))}
        </div>
      )}

      {convState.type === 'awaiting_continuation' && (
        <div className="continuation-progress" role="status" aria-live="polite">
          <div className="continuation-progress-header">
            <span>Compacting conversation...</span>
            <span className="continuation-progress-caption">Preparing a continuation</span>
          </div>
          <div className="continuation-progress-bar" aria-hidden="true">
            <div className="continuation-progress-bar-fill" />
          </div>
          <p className="continuation-progress-text">Phoenix is generating a continuation summary to preserve context for a new conversation.</p>
        </div>
      )}

      {isDragOver && (
        <div className="input-drop-affordance" aria-live="polite">
          Drop files to attach
        </div>
      )}

      <ImageAttachments images={images} onRemove={handleRemoveImage} />
      {files.length > 0 && (
        <div className="file-attachments" aria-label="Attached files">
          {files.map((file, index) => (
            <div key={`${file.stored_path}-${index}`} className="file-attachment-chip">
              <span className="file-attachment-icon">📎</span>
              <span className="file-attachment-name" title={file.stored_path}>{file.original_name}</span>
              <span className="file-attachment-size">{formatBytes(file.size_bytes)}</span>
              <button
                type="button"
                className="file-attachment-remove"
                onClick={() => handleRemoveFile(index)}
                title="Remove attachment"
                aria-label={`Remove attachment ${file.original_name}`}
              >
                x
              </button>
            </div>
          ))}
        </div>
      )}
      {isUploadingFiles && <div className="input-uploading-files">Uploading attachments...</div>}

      {/* Hidden file input for image attachments */}
      <input
        ref={fileInputRef}
        type="file"
        accept={SUPPORTED_IMAGE_TYPES.join(',')}
        multiple
        onChange={handleFileChange}
        style={{ display: 'none' }}
      />

      {/* Inline autocomplete dropdown (REQ-IR-004) */}
      <div className="iac-container">
        {ir.dropdown}
      </div>

      {/* Skill argument hint ghost text (REQ-IR-005) */}
      {ir.skillArgumentHint && !ir.expansionError && (
        <div className="input-skill-hint" aria-live="polite">
          <span className="input-skill-hint-text">{ir.skillArgumentHint}</span>
        </div>
      )}

      {/* Expansion error inline indicator (REQ-IR-007) */}
      {ir.expansionError && (
        <div className="input-expansion-error" role="alert">
          <span className="input-expansion-error-icon">x</span>
          <span className="input-expansion-error-text">{ir.expansionError}</span>
          <button
            className="input-expansion-error-dismiss"
            onClick={() => ir.setExpansionError(null)}
            title="Dismiss"
          >
            x
          </button>
        </div>
      )}

      {/* Textarea container -- Send/Stop button always inside */}
      <div className={`input-textarea-wrap${agentWorking ? ' input-textarea-wrap--working' : ''}`}>
        <textarea
          ref={textareaRef}
          id="message-input"
          placeholder={placeholder}
          rows={2}
          value={voiceBase !== null
            ? (voiceBase.trim()
                ? voiceBase.trimEnd() + (voiceInterim ? ' ' + voiceInterim : '')
                : voiceInterim)
            : draft}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          onPaste={handlePaste}
          onSelect={ir.onSelectionChange}
        />
        <div className="input-inline-actions">
          {!agentWorking && voiceSupported && (
            <VoiceRecorder
              onStart={handleVoiceStart}
              onEnd={handleVoiceEnd}
              onSpeech={handleVoiceFinal}
              onInterim={handleVoiceInterim}
              disabled={agentWorking}
            />
          )}
          {canCancel || isCancelling ? (
            <button
              className="input-stop-btn"
              onClick={onCancel}
              disabled={isCancelling || isOffline}
              title="Stop agent (Esc)"
            >
              {isCancelling ? 'Stopping...' : 'Stop'}
            </button>
          ) : (
            <button
              className="input-send-btn"
              onClick={handleSend}
              disabled={!sendEnabled}
              title="Enter to send"
            >
              Send
            </button>
          )}
        </div>
      </div>
    </footer>
  );
});

/**
 * Subscribes to the draft slice and feeds the presentational `<InputArea>`.
 * Hosting the subscription in this wrapper keeps the re-render scoped to
 * the composer subtree — sibling components don't see keystroke churn.
 */
export type ConnectedInputAreaProps = Omit<InputAreaProps, 'draft' | 'onDraftChange'> & {
  slug: string;
};

export const ConnectedInputArea = forwardRef<InputAreaHandle, ConnectedInputAreaProps>(
  function ConnectedInputArea({ slug, ...rest }, ref) {
    const draft = useDraftValue(slug);
    const { setDraft } = useDraftActions(slug);
    return <InputArea ref={ref} {...rest} draft={draft} onDraftChange={setDraft} />;
  },
);
