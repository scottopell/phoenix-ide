import type React from 'react';
import type { ConversationContentSearchHit } from '../../api';

// --- State Machine Types ---

export type ClosedState = { status: 'closed' };

export type SearchScope = 'global' | 'conversation-content' | 'conversation-slugs';

export type SearchStatus =
  | { kind: 'idle' }
  | { kind: 'awaiting-query' }
  | { kind: 'debouncing' }
  | { kind: 'loading' }
  | { kind: 'warming'; message: string }
  | { kind: 'error'; message: string }
  | { kind: 'ready' };

export type OpenState = {
  status: 'open';
  mode: 'search' | 'action';
  scope: SearchScope;
  query: string; // Without a mode or scope prefix
  rawInput: string; // Exact text in input field
  selectedIndex: number;
  results: PaletteItem[];
  searchStatus: SearchStatus;
};

export type PaletteState = ClosedState | OpenState;

export type PaletteEvent =
  | { type: 'OPEN' }
  | { type: 'CLOSE' }
  | { type: 'SET_QUERY'; rawInput: string }
  | { type: 'SEARCH_AWAITING_QUERY' }
  | { type: 'SEARCH_DEBOUNCING' }
  | { type: 'SEARCH_LOADING' }
  | { type: 'SEARCH_WARMING'; message: string }
  | { type: 'SEARCH_ERROR'; message: string }
  | { type: 'SET_RESULTS'; results: PaletteItem[] }
  | { type: 'SELECT_NEXT' }
  | { type: 'SELECT_PREV' }
  | { type: 'CONFIRM' };

// --- Source & Action Interfaces ---

interface BasePaletteItem {
  id: string;
  title: string;
  subtitle?: string;
  badge?: string;
  /** Match snippet for code search results. */
  snippet?: string;
  icon?: React.ReactNode;
  category: string;

  /** Match score for ranking (higher = better) */
  score?: number;
}

export type PaletteItem =
  | (BasePaletteItem & {
      sourceId: 'conversation-content';
      metadata: ConversationContentSearchHit;
    })
  | (BasePaletteItem & {
      sourceId: 'conversations' | 'code' | 'files' | 'actions';
      metadata?: unknown;
    });

export interface PaletteSource {
  id: string;
  category: string;
  /**
   * Return items matching query. Empty query = defaults/recents.
   * Async — callers must await and handle cancellation via AbortSignal.
   */
  search(query: string, signal?: AbortSignal): Promise<PaletteItem[]>;
  /** Handle item selection */
  onSelect(item: PaletteItem): void;
}

export interface PaletteAction {
  id: string;
  title: string;
  category?: string;
  shortcut?: string;
  icon?: React.ReactNode;
  handler: () => void;
}
