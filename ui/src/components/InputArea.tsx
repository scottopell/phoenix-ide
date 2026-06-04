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
} from 'react';
// Icon buttons removed from action row -- file browse via sidebar, image attach via paste/drag
import type { QueuedMessage } from '../hooks';
import { useDraftActions, useDraftValue, useScopedState, useInlineReferences } from '../hooks';
import type { ConversationState, ImageData } from '../api';
import { ExpansionError } from '../api';
import { canCancelConversationState, isAgentWorking, isCancellingState } from '../utils';
import { ImageAttachments } from './ImageAttachments';
import { VoiceRecorder, isWebSpeechSupported } from './VoiceInput';
import { SUPPORTED_IMAGE_TYPES, processImageFiles } from '../utils/images';

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
  convState: ConversationState;
  images: ImageData[];
  setImages: (images: ImageData[]) => void;
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
  onSend: (text: string, images: ImageData[]) => Promise<void> | void;
  onCancel: () => void;
  onRetry: (localId: string) => void;
  onDismissError?: (localId: string) => void;
}

export const InputArea = forwardRef<InputAreaHandle, InputAreaProps>(function InputArea({
  cwd,
  convState,
  images,
  setImages,
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
  const [voiceBase, setVoiceBase] = useScopedState<string | null>(cwd, null); // null = not recording
  const [voiceInterim, setVoiceInterim] = useScopedState(cwd, '');

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
    textareaRef,
    value: acValue,
    setValue: applyRefValue,
  });

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

  // =========================================================================
  // Send (with expansion error handling — REQ-IR-007)
  // =========================================================================

  const { reset: resetRefs, setExpansionError } = ir;

  const handleSend = useCallback(async () => {
    let text: string;
    if (voiceBase !== null) {
      text = voiceBase.trim() + (voiceInterim ? ' ' + voiceInterim.trim() : '');
    } else {
      text = draft.trim();
    }

    if (!text && images.length === 0) return;
    if (blocksComposerSend) return;
    // Allow send while agent is working — the server will queue it as a
    // steering message and deliver it when the conversation reaches Idle.

    // Close autocomplete and ghost text on send
    resetRefs();

    // Clear draft and images eagerly — the message is already queued and
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
    setExpansionError(null);

    try {
      await onSend(text, images);
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
      }
      // Non-expansion errors: draft is already cleared; the message queue
      // shows the failure with a retry button.
    }
  }, [
    voiceBase,
    voiceInterim,
    draft,
    images,
    blocksComposerSend,
    onSend,
    clearDraft,
    resetRefs,
    setExpansionError,
    setImages,
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
  const hasContent = displayedText.trim().length > 0 || voiceInterim.trim().length > 0 || images.length > 0;
  const canSend = !blocksComposerSend;
  const sendEnabled = canSend && hasContent && !ir.expansionError;

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
    <footer id="input-area">
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

      <ImageAttachments images={images} onRemove={handleRemoveImage} />

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
