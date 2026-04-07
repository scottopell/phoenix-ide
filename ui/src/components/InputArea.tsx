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
import { useDraft } from '../hooks';
import type { ConversationState, ImageData, SkillEntry } from '../api';
import { api, ExpansionError } from '../api';
import { isAgentWorking, isCancellingState } from '../utils';
import { ImageAttachments } from './ImageAttachments';
import { VoiceRecorder, isWebSpeechSupported } from './VoiceInput';
import { SUPPORTED_IMAGE_TYPES, processImageFiles } from '../utils/images';
import {
  InlineAutocomplete,
  detectTrigger,
  applyCompletion,
} from './InlineAutocomplete';
import type { AutocompleteItem, TriggerState } from './InlineAutocomplete';
import { fuzzyMatch } from './CommandPalette/fuzzyMatch';

export interface InputAreaHandle {
  appendToDraft: (text: string) => void;
  setDraft: (text: string) => void;
}

interface InputAreaProps {
  conversationId: string | undefined;
  convState: ConversationState;
  images: ImageData[];
  setImages: (images: ImageData[]) => void;
  isOffline: boolean;
  queuedMessages: QueuedMessage[];
  /** Conversation mode label (e.g. "Explore", "Work", "Direct") */
  convModeLabel?: string | undefined;
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
  conversationId,
  convState,
  images,
  setImages,
  isOffline,
  queuedMessages,
  convModeLabel,
  onSend,
  onCancel,
  onRetry,
  onDismissError,
}, ref) {
  const agentWorking = isAgentWorking(convState);
  const isCancelling = isCancellingState(convState);
  const [draft, setDraft, clearDraft] = useDraft(conversationId);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const voiceSupported = isWebSpeechSupported();

  useImperativeHandle(ref, () => ({
    appendToDraft: (text: string) => {
      setDraft(draft.trim() ? draft + '\n\n' + text : text);
    },
    setDraft: (text: string) => {
      setDraft(text);
    },
  }), [draft, setDraft]);

  // Listen for external insert-draft events (e.g., from SkillViewer)
  useEffect(() => {
    const handler = (e: Event) => {
      const text = (e as CustomEvent<{ text: string }>).detail?.text;
      if (text) {
        setDraft(text);
        textareaRef.current?.focus();
      }
    };
    window.addEventListener('phoenix:insert-draft', handler);
    return () => window.removeEventListener('phoenix:insert-draft', handler);
  }, [setDraft]);

  // Voice input: base text (accumulated finals) + interim (current partial)
  const [voiceBase, setVoiceBase] = useState<string | null>(null); // null = not recording
  const [voiceInterim, setVoiceInterim] = useState('');

  // =========================================================================
  // Inline autocomplete state (REQ-IR-004, REQ-IR-005)
  // =========================================================================

  /** Active trigger state — null when no trigger is open */
  const [activeTrigger, setActiveTrigger] = useState<TriggerState | null>(null);
  /** Candidate items fetched from the server */
  const [acItems, setAcItems] = useState<AutocompleteItem[]>([]);
  /** Inline error when an @ref or /skill fails to expand (REQ-IR-007) */
  const [expansionError, setExpansionError] = useState<string | null>(null);
  /** Ref to abort any in-flight search request */
  const searchAbortRef = useRef<AbortController | null>(null);
  /** Debounce timer for search */
  const searchTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** Cached skill list for the current conversation (REQ-IR-005) */
  const [skillItems, setSkillItems] = useState<SkillEntry[]>([]);
  /** Argument hint ghost text shown after a skill is selected (REQ-IR-005) */
  const [skillArgumentHint, setSkillArgumentHint] = useState<string | null>(null);

  // =========================================================================
  // File search (REQ-IR-004)
  // =========================================================================

  const fetchFileItems = useCallback(
    async (query: string) => {
      if (!conversationId) return;

      // Abort previous request
      searchAbortRef.current?.abort();
      const controller = new AbortController();
      searchAbortRef.current = controller;

      try {
        const result = await api.searchConversationFiles(
          conversationId,
          query,
          50,
          controller.signal,
        );
        const items: AutocompleteItem[] = result.items.map((entry) => ({
          id: entry.path,
          label: entry.path,
          ...(entry.is_text_file ? {} : { subtitle: 'binary' }),
          metadata: entry,
        }));
        setAcItems(items);
      } catch (err) {
        // Ignore abort errors
        if (err instanceof Error && err.name === 'AbortError') return;
        console.warn('File search failed:', err);
        setAcItems([]);
      }
    },
    [conversationId],
  );

  // =========================================================================
  // Skill search (REQ-IR-005)
  // =========================================================================

  /** Fetch and cache available skills for this conversation (once per session) */
  const fetchSkillItems = useCallback(async () => {
    if (!conversationId) return;
    try {
      const result = await api.listConversationSkills(conversationId);
      setSkillItems(result.skills);
    } catch (err) {
      console.warn('Skill list failed:', err);
      setSkillItems([]);
    }
  }, [conversationId]);

  // Debounced fetch when trigger/query changes
  useEffect(() => {
    if (!activeTrigger) {
      setAcItems([]);
      return;
    }

    if (activeTrigger.mode === 'skill') {
      // Skills are fetched once (cached in skillItems); map to AutocompleteItems here
      if (skillItems.length === 0) {
        void fetchSkillItems();
      }
      const items: AutocompleteItem[] = skillItems.map((s) => ({
        id: s.name,
        label: s.name,
        subtitle: s.description,
        metadata: s,
      }));
      setAcItems(items);
      return;
    }

    if (activeTrigger.mode !== 'expand' && activeTrigger.mode !== 'path') {
      setAcItems([]);
      return;
    }

    if (searchTimerRef.current) clearTimeout(searchTimerRef.current);
    searchTimerRef.current = setTimeout(() => {
      void fetchFileItems(activeTrigger.query);
    }, 80);

    return () => {
      if (searchTimerRef.current) clearTimeout(searchTimerRef.current);
    };
  }, [activeTrigger, fetchFileItems, fetchSkillItems, skillItems]);

  // When skillItems updates after a fetch, refresh acItems if skill trigger is still active
  useEffect(() => {
    if (activeTrigger?.mode !== 'skill') return;
    const items: AutocompleteItem[] = skillItems.map((s) => ({
      id: s.name,
      label: s.name,
      subtitle: s.description,
      metadata: s,
    }));
    setAcItems(items);
  }, [skillItems, activeTrigger?.mode]);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      searchAbortRef.current?.abort();
    };
  }, []);

  // =========================================================================
  // Trigger detection on text change
  // =========================================================================

  const handleTextChange = useCallback(
    (newValue: string) => {
      // Clear expansion error on edit
      setExpansionError(null);

      const ta = textareaRef.current;
      const cursor = ta?.selectionStart ?? newValue.length;
      const trigger = detectTrigger(newValue, cursor);
      setActiveTrigger(trigger);
    },
    [],
  );

  // =========================================================================
  // Autocomplete selection
  // =========================================================================

  const handleAcSelect = useCallback(
    (item: AutocompleteItem) => {
      if (!activeTrigger) return;

      const currentValue = voiceBase !== null ? voiceBase : draft;

      let replacement: string;
      if (activeTrigger.mode === 'expand') {
        replacement = `@${item.label} `;
      } else if (activeTrigger.mode === 'skill') {
        // Insert `/skill-name ` with a trailing space (REQ-IR-005)
        replacement = `/${item.label} `;
        // Show argument hint ghost text if the skill has one
        const skill = item.metadata as SkillEntry | undefined;
        if (skill?.argument_hint) {
          setSkillArgumentHint(skill.argument_hint);
        } else {
          setSkillArgumentHint(null);
        }
      } else {
        // path mode — trailing space dismisses autocomplete popup
        replacement = `./${item.label} `;
      }

      const { newValue, newCursorPos } = applyCompletion(currentValue, activeTrigger, replacement);

      if (voiceBase !== null) {
        setVoiceBase(newValue);
      } else {
        setDraft(newValue);
      }

      setActiveTrigger(null);
      setAcItems([]);

      // Restore cursor position after React re-render
      requestAnimationFrame(() => {
        const ta = textareaRef.current;
        if (ta) {
          ta.setSelectionRange(newCursorPos, newCursorPos);
          ta.focus();
        }
      });
    },
    [activeTrigger, voiceBase, draft, setDraft, setVoiceBase],
  );

  const handleAcDismiss = useCallback(() => {
    setActiveTrigger(null);
    setAcItems([]);
  }, []);

  // Clear argument hint when the user types past the skill name or clears input
  useEffect(() => {
    if (skillArgumentHint === null) return;
    const text = voiceBase !== null ? voiceBase : draft;
    // Keep hint visible while input starts with a /skill-name pattern;
    // dismiss it once the user has added arguments (text after the first token has content)
    const match = /^\/\S+\s(.*)/.exec(text.trimStart());
    if (match !== null && (match[1] ?? '').length > 0) {
      // User has started typing arguments — dismiss the hint
      setSkillArgumentHint(null);
    } else if (!text.trimStart().startsWith('/')) {
      setSkillArgumentHint(null);
    }
  }, [draft, voiceBase, skillArgumentHint]);

  // =========================================================================
  // Keyboard handling (merged with autocomplete nav)
  // =========================================================================

  const [acSelectedIndex, setAcSelectedIndex] = useState(0);

  const filteredItems = fuzzyMatch(acItems, activeTrigger?.query ?? '', (item) => item.label);

  useEffect(() => {
    setAcSelectedIndex(0);
  }, [activeTrigger?.query]);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLTextAreaElement>) => {
      // When autocomplete is open, intercept navigation and confirmation keys
      if (activeTrigger && filteredItems.length > 0) {
        if (e.key === 'ArrowDown') {
          e.preventDefault();
          setAcSelectedIndex((i) => Math.min(i + 1, filteredItems.length - 1));
          return;
        }
        if (e.key === 'ArrowUp') {
          e.preventDefault();
          setAcSelectedIndex((i) => Math.max(i - 1, 0));
          return;
        }
        if (e.key === 'Tab') {
          const item = filteredItems[acSelectedIndex] ?? filteredItems[0];
          if (item !== undefined) {
            e.preventDefault();
            handleAcSelect(item);
            return;
          }
        }
        if (e.key === 'Escape') {
          e.preventDefault();
          handleAcDismiss();
          return;
        }
        // Enter with autocomplete open: if item selected, complete; otherwise fall through to send
        if (e.key === 'Enter' && !e.shiftKey) {
          const item = filteredItems[acSelectedIndex] ?? filteredItems[0];
          if (item !== undefined) {
            e.preventDefault();
            handleAcSelect(item);
            return;
          }
        }
      }

      // Default: Enter without shift sends message
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        handleSend();
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [activeTrigger, filteredItems, acSelectedIndex, handleAcSelect, handleAcDismiss],
  );

  // =========================================================================
  // Auto-resize
  // =========================================================================

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

  const handleSend = useCallback(async () => {
    let text: string;
    if (voiceBase !== null) {
      text = voiceBase.trim() + (voiceInterim ? ' ' + voiceInterim.trim() : '');
    } else {
      text = draft.trim();
    }

    if (!text && images.length === 0) return;
    if (agentWorking && !isOffline) return;

    // Close autocomplete and ghost text on send
    setActiveTrigger(null);
    setAcItems([]);
    setSkillArgumentHint(null);

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
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [voiceBase, voiceInterim, draft, images, agentWorking, isOffline, onSend, clearDraft]);

  // =========================================================================
  // Voice input
  // =========================================================================

  const handleVoiceStart = useCallback(() => {
    setVoiceBase(draft);
    setVoiceInterim('');
  }, [draft]);

  const handleVoiceEnd = useCallback(() => {
    setVoiceBase(prev => {
      if (prev !== null) {
        setDraft(prev);
      }
      return null;
    });
    setVoiceInterim('');
  }, [setDraft]);

  const handleVoiceFinal = useCallback((text: string) => {
    if (!text) return;
    setVoiceBase(prev => {
      if (prev === null) return null;
      return prev.trim() ? prev.trimEnd() + ' ' + text : text;
    });
    setVoiceInterim('');
  }, []);

  const handleVoiceInterim = useCallback((text: string) => {
    setVoiceInterim(text);
  }, []);

  // =========================================================================
  // Derived state
  // =========================================================================

  const failedMessages = queuedMessages.filter(m => m.status === 'failed');
  const displayedText = voiceBase !== null ? voiceBase : draft;
  const hasContent = displayedText.trim().length > 0 || voiceInterim.trim().length > 0 || images.length > 0;
  const sendEnabled = (!agentWorking || isOffline) && hasContent && !expansionError;

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
    : agentWorking
      ? 'Agent working... draft your next message'
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
        <InlineAutocomplete
          mode={activeTrigger?.mode ?? 'expand'}
          query={activeTrigger?.query ?? ''}
          items={acItems}
          selectedIndex={acSelectedIndex}
          onSelect={handleAcSelect}
          visible={activeTrigger !== null}
        />
      </div>

      {/* Skill argument hint ghost text (REQ-IR-005) */}
      {skillArgumentHint && !expansionError && (
        <div className="input-skill-hint" aria-live="polite">
          <span className="input-skill-hint-text">{skillArgumentHint}</span>
        </div>
      )}

      {/* Expansion error inline indicator (REQ-IR-007) */}
      {expansionError && (
        <div className="input-expansion-error" role="alert">
          <span className="input-expansion-error-icon">x</span>
          <span className="input-expansion-error-text">{expansionError}</span>
          <button
            className="input-expansion-error-dismiss"
            onClick={() => setExpansionError(null)}
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
          onChange={(e) => {
            const newVal = e.target.value;
            if (voiceBase !== null) {
              setVoiceBase(newVal);
              setVoiceInterim('');
            } else {
              setDraft(newVal);
            }
            handleTextChange(newVal);
          }}
          onKeyDown={handleKeyDown}
          onPaste={handlePaste}
          onSelect={() => {
            // Re-detect trigger on cursor movement (arrow keys, click)
            const ta = textareaRef.current;
            if (ta) {
              const currentVal = voiceBase !== null ? voiceBase : draft;
              const trigger = detectTrigger(currentVal, ta.selectionStart);
              setActiveTrigger(trigger);
            }
          }}
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
          {agentWorking ? (
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

