/* eslint-disable react-refresh/only-export-components */
/**
 * InlineAutocomplete — shared overlay for inline reference triggers.
 *
 * Supports three trigger modes (REQ-IR-004, REQ-IR-005, REQ-IR-008):
 *   - "expand"  (@)  — file reference that will be expanded at send time
 *   - "path"    (./) — file path inserted as literal text, no server expansion
 *   - "skill"   (/)  — skill invocation (task 570, not implemented here)
 *
 * This file intentionally exports both the component and pure utility functions
 * (detectTrigger, applyCompletion) so that InputArea can import them together.
 * The react-refresh warning is suppressed because the utilities are closely
 * coupled to the component and not shared elsewhere.
 */

import {
  useEffect,
  useRef,
} from 'react';
import { fuzzyMatch } from './CommandPalette/fuzzyMatch';

// ============================================================================
// Types
// ============================================================================

/** Trigger mode — determines display prefix and send-time behaviour */
export type AutocompleteMode = 'expand' | 'path' | 'skill';

/** A single candidate item shown in the dropdown */
export interface AutocompleteItem {
  /** Unique key */
  id: string;
  /** Display label */
  label: string;
  /** Optional secondary line (e.g. description for skills) */
  subtitle?: string | undefined;
  /** Opaque metadata carried through to onSelect */
  metadata?: unknown;
}

interface InlineAutocompleteProps {
  /** Trigger mode — controls prefix and display */
  mode: AutocompleteMode;
  /** Current query string typed after the trigger character */
  query: string;
  /** Full candidate list (unfiltered; component applies fuzzy matching) */
  items: AutocompleteItem[];
  /** Index of the selected item (driven by parent) */
  selectedIndex: number;
  /** Called when the user clicks an item */
  onSelect: (item: AutocompleteItem) => void;
  /** Whether the dropdown should be shown at all */
  visible: boolean;
}

// ============================================================================
// Helpers
// ============================================================================

const TRIGGER_PREFIX: Record<AutocompleteMode, string> = {
  expand: '@',
  path: './',
  skill: '/',
};

const MODE_HINT: Record<AutocompleteMode, string> = {
  expand: 'file will be included',
  path: 'path reference',
  skill: 'skill',
};

// ============================================================================
// Component
// ============================================================================

export function InlineAutocomplete({
  mode,
  query,
  items,
  selectedIndex,
  onSelect,
  visible,
}: InlineAutocompleteProps) {
  const listRef = useRef<HTMLDivElement>(null);

  // Filter and rank items using fuzzy matching
  const filtered = fuzzyMatch(items, query, (item) => item.label);

  // Scroll selected item into view
  useEffect(() => {
    if (!listRef.current) return;
    const selected = listRef.current.querySelector<HTMLElement>('.iac-item.selected');
    selected?.scrollIntoView({ block: 'nearest' });
  }, [selectedIndex]);

  if (!visible || filtered.length === 0) {
    return null;
  }

  const prefix = TRIGGER_PREFIX[mode];
  const hint = MODE_HINT[mode];

  return (
    <div className="iac-dropdown" role="listbox" aria-label={`${prefix} autocomplete`}>
      <div className="iac-header">
        <span className="iac-trigger-prefix">{prefix}</span>
        <span className="iac-hint">{hint}</span>
      </div>
      <div className="iac-list" ref={listRef}>
        {filtered.slice(0, 12).map((item, idx) => (
          <button
            key={item.id}
            className={`iac-item ${idx === selectedIndex ? 'selected' : ''}`}
            role="option"
            aria-selected={idx === selectedIndex}
            onClick={() => onSelect(item)}
            type="button"
          >
            <span className="iac-item-label">{item.label}</span>
            {item.subtitle !== undefined && (
              <span className="iac-item-subtitle">{item.subtitle}</span>
            )}
          </button>
        ))}
      </div>
    </div>
  );
}

// ============================================================================
// Trigger detection utilities
// ============================================================================

/** Result of scanning the textarea value for an active trigger */
export interface TriggerState {
  mode: AutocompleteMode;
  /** The partial query typed after the trigger character */
  query: string;
  /**
   * Start index (inclusive) of the trigger token (including the trigger char)
   * in the textarea value string.
   */
  triggerStart: number;
  /**
   * End index (exclusive) of the trigger token — equals the current cursor position.
   */
  triggerEnd: number;
}

/**
 * Detect whether the text before `cursorPos` ends in an active trigger pattern.
 *
 * Returns `null` when no trigger is active.
 *
 * Rules:
 *   - `@<partial>` anywhere (no whitespace in partial)          → expand mode
 *   - `<word_boundary>./<partial>` anywhere                     → path mode
 */
export function detectTrigger(value: string, cursorPos: number): TriggerState | null {
  const beforeCursor = value.slice(0, cursorPos);

  // ---- @expand trigger -------------------------------------------------------
  const atIdx = findLastTriggerChar(beforeCursor, '@');
  if (atIdx !== null) {
    const query = beforeCursor.slice(atIdx + 1);
    if (!containsWhitespace(query)) {
      return {
        mode: 'expand',
        query,
        triggerStart: atIdx,
        triggerEnd: cursorPos,
      };
    }
  }

  // ---- ./ path trigger -------------------------------------------------------
  const dotSlashIdx = findLastDotSlashTrigger(beforeCursor);
  if (dotSlashIdx !== null) {
    const query = beforeCursor.slice(dotSlashIdx + 2); // skip "./"
    if (!containsWhitespace(query)) {
      return {
        mode: 'path',
        query,
        triggerStart: dotSlashIdx,
        triggerEnd: cursorPos,
      };
    }
  }

  return null;
}

function containsWhitespace(s: string): boolean {
  return s.includes(' ') || s.includes('\t') || s.includes('\n');
}

/** Find last occurrence of `triggerChar` that is at start or preceded by whitespace */
function findLastTriggerChar(text: string, triggerChar: string): number | null {
  let idx = text.lastIndexOf(triggerChar);
  while (idx !== -1) {
    const prevChar = idx > 0 ? text[idx - 1] : null;
    if (prevChar === null || prevChar === ' ' || prevChar === '\t' || prevChar === '\n') {
      return idx;
    }
    idx = text.lastIndexOf(triggerChar, idx - 1);
  }
  return null;
}

/** Find last `./` that is at start or preceded by whitespace */
function findLastDotSlashTrigger(text: string): number | null {
  // Search backwards through the text for valid `./` trigger positions
  let searchEnd = text.length;
  while (searchEnd > 0) {
    const slice = text.slice(0, searchEnd);
    const idx = slice.lastIndexOf('./');
    if (idx === -1) return null;

    const prevChar = idx > 0 ? text[idx - 1] : null;
    if (prevChar === null || prevChar === ' ' || prevChar === '\t' || prevChar === '\n') {
      return idx;
    }
    // Not a valid trigger position — move search window back past this occurrence
    searchEnd = idx;
  }
  return null;
}

/**
 * Replace the trigger token in `value` with `replacement`.
 *
 * `replacement` is the full string to insert including the trigger prefix
 * (e.g. `@src/main.rs` or `./src/main.rs`).
 */
export function applyCompletion(
  value: string,
  trigger: TriggerState,
  replacement: string,
): { newValue: string; newCursorPos: number } {
  const newValue =
    value.slice(0, trigger.triggerStart) + replacement + value.slice(trigger.triggerEnd);
  const newCursorPos = trigger.triggerStart + replacement.length;
  return { newValue, newCursorPos };
}

