import type React from 'react';

// --- State Machine Types ---

export type ClosedState = { status: 'closed' };

export type OpenState = {
  status: 'open';
  mode: 'search' | 'action';
  query: string; // Without the '>' prefix in action mode
  rawInput: string; // Exact text in input field
  selectedIndex: number;
  results: PaletteItem[];
};

export type PaletteState = ClosedState | OpenState;

export type PaletteEvent =
  | { type: 'OPEN' }
  | { type: 'CLOSE' }
  | { type: 'SET_QUERY'; rawInput: string }
  | { type: 'SELECT_NEXT' }
  | { type: 'SELECT_PREV' }
  | { type: 'CONFIRM' };

// --- Source & Action Interfaces ---

export interface PaletteItem {
  id: string;
  title: string;
  subtitle?: string;
  icon?: React.ReactNode;
  category: string;
  metadata?: unknown;
  /** Match score for ranking (higher = better) */
  score?: number;
}

export interface PaletteSource {
  id: string;
  category: string;
  /** Return items matching query. Empty query = defaults/recents. */
  search(query: string): PaletteItem[];
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
