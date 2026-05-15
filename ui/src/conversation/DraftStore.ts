import { RoutedStore } from './RoutedStore';

/**
 * Per-slug draft state. Lives in a dedicated store so server-driven
 * conversation state and client-typed draft state don't share a
 * subscription target — without that separation, every keystroke would
 * invalidate `ConversationPageContent`'s whole-atom snapshot and
 * re-render the message list, terminal, and breadcrumbs.
 *
 * Slug-keying replaces the `expectedConversationId` guard the
 * conversation atom uses for client-originated actions: a stale-closure
 * dispatch from a previous conversation's effect lands on the old slug's
 * draft and can't corrupt the active one.
 */
export interface DraftAtom {
  draft: string;
}

export type DraftAction =
  | { type: 'set_draft'; text: string }
  | { type: 'append_draft'; text: string }
  | { type: 'clear_draft' };

export function draftReducer(atom: DraftAtom, action: DraftAction): DraftAtom {
  switch (action.type) {
    case 'set_draft':
      if (atom.draft === action.text) return atom;
      return { draft: action.text };

    case 'append_draft': {
      if (!action.text) return atom;
      // Read-modify-write inside the reducer so concurrent appends
      // (terminal selection + prose-reader notes in quick succession)
      // compose deterministically.
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

export class DraftStore extends RoutedStore<string, DraftAtom, DraftAction> {
  constructor() {
    super(() => ({ draft: '' }), draftReducer);
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
