import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  type KeyboardEvent,
  type RefObject,
} from 'react';
import { createElement, type ReactNode } from 'react';
import { api } from '../api';
import type { SkillEntry } from '../api';
import { useScopedState } from './useScopedState';
import {
  InlineAutocomplete,
  detectTrigger,
  applyCompletion,
} from '../components/InlineAutocomplete';
import type { AutocompleteItem, TriggerState } from '../components/InlineAutocomplete';
import { fuzzyMatch } from '../components/CommandPalette/fuzzyMatch';

/**
 * Inline reference autocomplete engine (REQ-IR-004, REQ-IR-005, REQ-IR-008),
 * scoped to a working directory rather than a conversation.
 *
 * A conversation has a single immutable `cwd`, and the new-conversation
 * composer picks a directory before any conversation exists — so the
 * candidate set for `@file` / `./path` / `/skill` is a pure function of that
 * directory. Keying the engine on `cwd` lets the same composer behaviour run
 * in both places, and resolves candidates against the same root that
 * `message_expander::expand` uses at send time.
 */
export interface UseInlineReferencesParams {
  /**
   * Directory to resolve `@file` / `./path` / `/skill` candidates against.
   * `undefined`/empty disables fetching (the dropdown simply never populates).
   * The skill/file candidate caches are keyed on this.
   */
  cwd: string | undefined;
  /**
   * Creation mode of the composer's workflow. With `baseBranch`, branch/managed
   * modes discover candidates from the chosen branch's committed tree (what the
   * conversation's worktree will hold) rather than the live `cwd`, so a
   * suggestion always matches what create-time expansion can resolve. Omitted ⇒
   * Direct (resolve against `cwd`). An in-conversation composer leaves this
   * unset: its conversation already resolves against its own `cwd`.
   */
  mode?: 'direct' | 'managed' | 'branch';
  /** Branch the conversation will be created on, for branch/managed modes. */
  baseBranch?: string | null;
  /**
   * Identity of the composer this engine belongs to: a conversation id for an
   * in-conversation composer, a stable key for the new-conversation composer.
   * Transient UI state (active trigger, in-flight results, expansion error,
   * skill hint, selection index) resets when this changes. Without it, two
   * conversations sharing a `cwd` — rendered by the same component tree across
   * a route-param change — would leak one's stale expansion error (and dropdown)
   * into the other, wrongly disabling Send.
   */
  scopeKey: string | undefined;
  /** The composer textarea — used to read caret position for trigger detection. */
  textareaRef: RefObject<HTMLTextAreaElement>;
  /** Current composer text (already merged with any in-progress voice input). */
  value: string;
  /** Apply a completion's replacement text back into the composer. */
  setValue: (next: string) => void;
}

export interface InlineReferences {
  /** True while a trigger is active (an autocomplete dropdown may be shown). */
  active: boolean;
  /** Argument-hint ghost text shown after a skill is chosen (REQ-IR-005). */
  skillArgumentHint: string | null;
  /** Inline expansion error (REQ-IR-007); set by the send path, cleared on edit. */
  expansionError: string | null;
  setExpansionError: (e: string | null) => void;
  /** Notify the engine the composer text changed (keystroke, voice, paste). */
  onValueChange: (next: string) => void;
  /** Notify the engine the caret moved (arrow keys, click). */
  onSelectionChange: () => void;
  /**
   * Intercept autocomplete navigation/confirmation keys. Returns `true` when
   * the key was consumed (the caller should not run its own handling).
   */
  onKeyDown: (e: KeyboardEvent<HTMLTextAreaElement>) => boolean;
  /** Close the dropdown and clear ghost text (call when a message is sent). */
  reset: () => void;
  /** The autocomplete dropdown overlay, ready to render (null when hidden). */
  dropdown: ReactNode;
}

export function useInlineReferences({
  cwd,
  mode,
  baseBranch,
  scopeKey,
  textareaRef,
  value,
  setValue,
}: UseInlineReferencesParams): InlineReferences {
  // Candidates depend on the directory AND the ref (branch/managed resolve
  // against a branch's committed tree, not `cwd`). The skill cache and the
  // in-flight staleness guards key on this composite so switching workflow or
  // branch refetches against the new root. JSON-encoded so the component parts
  // can't collide.
  const discoveryOpts = useMemo(() => ({ mode, baseBranch }), [mode, baseBranch]);
  const discoveryKey = cwd ? JSON.stringify([cwd, mode ?? 'direct', baseBranch ?? '']) : undefined;
  // Transient UI state is keyed on the composer identity (`scopeKey`) so it
  // resets when the composer switches conversations, even within one `cwd`.
  /** Active trigger state — null when no trigger is open. */
  const [activeTrigger, setActiveTrigger] = useScopedState<TriggerState | null>(scopeKey, null);
  /**
   * File search results fetched from the server. Skill candidates are NOT
   * stored here — they're derived during render from `skillItems` (see the
   * `acItems` useMemo below), avoiding the derived-state-in-effect anti-pattern.
   */
  const [fileAcItems, setFileAcItems] = useScopedState<AutocompleteItem[]>(scopeKey, []);
  /** Inline error when an @ref or /skill fails to expand (REQ-IR-007). */
  const [expansionError, setExpansionError] = useScopedState<string | null>(scopeKey, null);
  /** Argument hint ghost text shown after a skill is selected (REQ-IR-005). */
  const [skillArgumentHint, setSkillArgumentHint] = useScopedState<string | null>(scopeKey, null);
  const [acSelectedIndex, setAcSelectedIndex] = useScopedState(scopeKey, 0);

  // The skill catalog is a property of the resolution root (directory + ref),
  // so it is keyed on `discoveryKey` and shared across composers on the same root.
  /** Cached skill list for the current resolution root (REQ-IR-005). */
  const [skillItems, setSkillItems] = useScopedState<SkillEntry[]>(discoveryKey, []);

  /** Aborts any in-flight file search request. */
  const searchAbortRef = useRef<AbortController | null>(null);
  /** Debounce timer for file search. */
  const searchTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** Guards against duplicate in-flight skill fetches. Holds the `discoveryKey`
   *  of the in-flight request (undefined when idle) so a fetch for root A does
   *  not suppress the fetch for root B after a switch. */
  const fetchingSkillsKeyRef = useRef<string | undefined>(undefined);
  /**
   * Latest requested `discoveryKey`. A fetch issued for root A may resolve after
   * the composer has switched to root B; comparing against this ref lets the
   * late response be discarded instead of populating B with A's results.
   */
  const latestKeyRef = useRef(discoveryKey);
  latestKeyRef.current = discoveryKey;

  /**
   * Autocomplete items to display, derived from the active trigger mode: skill
   * triggers map `skillItems` at render time; file triggers use the results
   * stored in `fileAcItems` by the debounced fetcher.
   */
  const acItems = useMemo<AutocompleteItem[]>(() => {
    if (!activeTrigger) return [];
    if (activeTrigger.mode === 'skill') {
      return skillItems.map((s) => ({
        id: s.name,
        label: s.name,
        subtitle: s.description,
        metadata: s,
      }));
    }
    if (activeTrigger.mode === 'expand' || activeTrigger.mode === 'path') {
      return fileAcItems;
    }
    return [];
  }, [activeTrigger, skillItems, fileAcItems]);

  const fetchFileItems = useCallback(
    async (query: string) => {
      if (!cwd) return;

      searchAbortRef.current?.abort();
      const controller = new AbortController();
      searchAbortRef.current = controller;

      try {
        const result = await api.searchProjectFiles(cwd, query, 50, discoveryOpts, controller.signal);
        // Drop the response if the resolution root changed while it was in flight.
        if (latestKeyRef.current !== discoveryKey) return;
        const items: AutocompleteItem[] = result.items.map((entry) => ({
          id: entry.path,
          label: entry.path,
          ...(entry.viewer.kind === 'opaque' ? { subtitle: 'binary' } : {}),
          metadata: entry,
        }));
        setFileAcItems(items);
      } catch (err) {
        if (err instanceof Error && err.name === 'AbortError') return;
        console.warn('File search failed:', err);
        setFileAcItems([]);
      }
    },
    [cwd, discoveryKey, discoveryOpts, setFileAcItems],
  );

  /** Fetch and cache available skills for this resolution root (once per key). */
  const fetchSkillItems = useCallback(async () => {
    if (!cwd) return;
    if (fetchingSkillsKeyRef.current === discoveryKey) return;
    fetchingSkillsKeyRef.current = discoveryKey;
    try {
      const result = await api.listProjectSkills(cwd, discoveryOpts);
      // Drop the response if the resolution root changed while it was in flight.
      if (latestKeyRef.current !== discoveryKey) return;
      setSkillItems(result.skills);
    } catch (err) {
      console.warn('Skill list failed:', err);
      if (latestKeyRef.current === discoveryKey) setSkillItems([]);
    } finally {
      if (fetchingSkillsKeyRef.current === discoveryKey) fetchingSkillsKeyRef.current = undefined;
    }
  }, [cwd, discoveryKey, discoveryOpts, setSkillItems]);

  // When autocomplete is disabled (no `cwd`), tear down any open dropdown and
  // stale file results so candidates fetched against a previous root can't stay
  // visible or be inserted into a context where they no longer resolve.
  useEffect(() => {
    if (!cwd) {
      setActiveTrigger(null);
      setFileAcItems([]);
    }
  }, [cwd, setActiveTrigger, setFileAcItems]);

  // Fire side effects (fetch) on trigger change. No state derivation here —
  // `acItems` is computed during render via the useMemo above.
  useEffect(() => {
    if (!activeTrigger) {
      setFileAcItems([]);
      return;
    }

    if (activeTrigger.mode === 'skill') {
      if (skillItems.length === 0) {
        void fetchSkillItems();
      }
      return;
    }

    if (activeTrigger.mode !== 'expand' && activeTrigger.mode !== 'path') {
      setFileAcItems([]);
      return;
    }

    if (searchTimerRef.current) clearTimeout(searchTimerRef.current);
    searchTimerRef.current = setTimeout(() => {
      void fetchFileItems(activeTrigger.query);
    }, 80);

    return () => {
      if (searchTimerRef.current) clearTimeout(searchTimerRef.current);
    };
  }, [activeTrigger, fetchFileItems, fetchSkillItems, skillItems.length, setFileAcItems]);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      searchAbortRef.current?.abort();
    };
  }, []);

  const onValueChange = useCallback(
    (newValue: string) => {
      // Clear expansion error on edit.
      setExpansionError(null);

      const ta = textareaRef.current;
      const cursor = ta?.selectionStart ?? newValue.length;
      setActiveTrigger(detectTrigger(newValue, cursor));
    },
    [setActiveTrigger, setExpansionError, textareaRef],
  );

  const onSelectionChange = useCallback(() => {
    // Re-detect trigger on cursor movement (arrow keys, click).
    const ta = textareaRef.current;
    if (ta) {
      setActiveTrigger(detectTrigger(value, ta.selectionStart));
    }
  }, [value, setActiveTrigger, textareaRef]);

  const handleAcSelect = useCallback(
    (item: AutocompleteItem) => {
      if (!activeTrigger) return;

      let replacement: string;
      if (activeTrigger.mode === 'expand') {
        replacement = `@${item.label} `;
      } else if (activeTrigger.mode === 'skill') {
        replacement = `/${item.label} `;
        const skill = item.metadata as SkillEntry | undefined;
        setSkillArgumentHint(skill?.argument_hint ?? null);
      } else {
        // path mode — trailing space dismisses autocomplete popup
        replacement = `./${item.label} `;
      }

      const { newValue, newCursorPos } = applyCompletion(value, activeTrigger, replacement);
      setValue(newValue);
      setActiveTrigger(null);
      // fileAcItems is cleared by the trigger-effect when activeTrigger → null.
      requestAnimationFrame(() => {
        const ta = textareaRef.current;
        if (ta) {
          ta.setSelectionRange(newCursorPos, newCursorPos);
          ta.focus();
        }
      });
    },
    [activeTrigger, value, setValue, setActiveTrigger, setSkillArgumentHint, textareaRef],
  );

  // Clear argument hint when the user types past the skill name or clears input.
  useEffect(() => {
    if (skillArgumentHint === null) return;
    const text = value;
    const match = /^\/\S+\s(.*)/.exec(text.trimStart());
    if (match !== null && (match[1] ?? '').length > 0) {
      setSkillArgumentHint(null);
    } else if (!text.trimStart().startsWith('/')) {
      setSkillArgumentHint(null);
    }
  }, [value, skillArgumentHint, setSkillArgumentHint]);

  const filteredItems = useMemo(
    () => fuzzyMatch(acItems, activeTrigger?.query ?? '', (item) => item.label),
    [acItems, activeTrigger?.query],
  );

  useEffect(() => {
    setAcSelectedIndex(0);
  }, [activeTrigger?.query, setAcSelectedIndex]);

  const onKeyDown = useCallback(
    (e: KeyboardEvent<HTMLTextAreaElement>): boolean => {
      if (!activeTrigger || filteredItems.length === 0) return false;

      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setAcSelectedIndex((i) => Math.min(i + 1, filteredItems.length - 1));
        return true;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setAcSelectedIndex((i) => Math.max(i - 1, 0));
        return true;
      }
      if (e.key === 'Tab') {
        const item = filteredItems[acSelectedIndex] ?? filteredItems[0];
        if (item !== undefined) {
          e.preventDefault();
          handleAcSelect(item);
          return true;
        }
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        setActiveTrigger(null);
        return true;
      }
      // Enter with autocomplete open: complete if an item is selected,
      // otherwise let the caller fall through to send.
      if (e.key === 'Enter' && !e.shiftKey) {
        const item = filteredItems[acSelectedIndex] ?? filteredItems[0];
        if (item !== undefined) {
          e.preventDefault();
          handleAcSelect(item);
          return true;
        }
      }
      return false;
    },
    [activeTrigger, filteredItems, acSelectedIndex, handleAcSelect, setAcSelectedIndex, setActiveTrigger],
  );

  const reset = useCallback(() => {
    setActiveTrigger(null);
    setSkillArgumentHint(null);
  }, [setActiveTrigger, setSkillArgumentHint]);

  const dropdown = createElement(InlineAutocomplete, {
    mode: activeTrigger?.mode ?? 'expand',
    query: activeTrigger?.query ?? '',
    items: acItems,
    selectedIndex: acSelectedIndex,
    onSelect: handleAcSelect,
    visible: activeTrigger !== null,
  });

  return {
    active: activeTrigger !== null,
    skillArgumentHint,
    expansionError,
    setExpansionError,
    onValueChange,
    onSelectionChange,
    onKeyDown,
    reset,
    dropdown,
  };
}
