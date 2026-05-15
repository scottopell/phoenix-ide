import { RoutedStore } from './RoutedStore';

/**
 * Per-slug draft state. Lives in a dedicated store so the
 * `ConversationAtom` (server-driven state) and the draft (client-typed
 * state) don't share a subscription target. `ConversationPageContent`
 * subscribes to the conversation atom on mount; if the draft lived on
 * that atom, every keystroke would invalidate the whole-atom snapshot
 * and re-render the page (message list, terminal pane, breadcrumbs,
 * etc.) — Codex review on PR #92.
 */
export interface DraftAtom {
  draft: string;
}

export type DraftAction =
  | { type: 'set_draft'; text: string }
  | { type: 'append_draft'; text: string }
  | { type: 'clear_draft' };

export function createInitialDraft(): DraftAtom {
  return { draft: '' };
}

export function draftReducer(atom: DraftAtom, action: DraftAction): DraftAtom {
  switch (action.type) {
    case 'set_draft':
      if (atom.draft === action.text) return atom;
      return { draft: action.text };

    case 'append_draft': {
      if (!action.text) return atom;
      // Blank-line separator iff existing draft has visible content,
      // otherwise replace. The read-modify-write happens inside the
      // reducer so concurrent appends (terminal selection + prose-reader
      // notes in quick succession) compose deterministically.
      const next = atom.draft.trim()
        ? atom.draft + '\n\n' + action.text
        : action.text;
      return { draft: next };
    }

    case 'clear_draft':
      if (atom.draft === '') return atom;
      return { draft: '' };
  }
}

/**
 * Slug-keyed draft store. Routing-by-slug means a stale-closure dispatch
 * from a previous conversation's effect lands harmlessly on the old slug's
 * draft (which is just persistence — re-visiting that conversation
 * surfaces the typed text again); it can't corrupt the active draft. No
 * `expectedConversationId` guard needed.
 */
export class DraftStore extends RoutedStore<string, DraftAtom, DraftAction> {
  constructor() {
    super(createInitialDraft, draftReducer);
  }

  /**
   * Drop the draft atom for `slug`. Called from the
   * `phoenix:conversation-hard-deleted` cascade so a slug that the
   * server later reuses for a new conversation (REQ-VS-014) doesn't
   * surface the previous conversation's in-memory draft. The
   * localStorage entry is keyed by conversation id and cleared
   * separately at the cascade site.
   */
  remove(slug: string): void {
    this.removeAtom(slug);
  }
}
