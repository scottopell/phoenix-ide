import type { FileAttachment, ImageData } from '../api';
import { RoutedStore } from './RoutedStore';

/**
 * Per-slug draft state. Lives in a dedicated store so server-driven
 * conversation state and client-typed draft state don't share a
 * subscription target — without that separation, every keystroke would
 * invalidate `ConversationPageContent`'s whole-atom snapshot and
 * re-render the message list and terminal.
 *
 * Slug-keying replaces the `expectedConversationId` guard the
 * conversation atom uses for client-originated actions: a stale-closure
 * dispatch from a previous conversation's effect lands on the old slug's
 * draft and can't corrupt the active one.
 */
export interface FencedSendRecovery {
  text: string;
  restoreTo: 'draft' | 'voice';
  error?: string;
  images: ImageData[];
  files: FileAttachment[];
}

export interface DraftAtom {
  draft: string;
  fencedSendRecoveries: FencedSendRecovery[];
}

export type DraftAction =
  | { type: 'set_draft'; text: string }
  | { type: 'set_draft_if_empty'; text: string }
  | { type: 'append_draft'; text: string }
  | { type: 'clear_draft' }
  | { type: 'enqueue_fenced_send_recovery'; recovery: FencedSendRecovery }
  | { type: 'shift_fenced_send_recovery' };

export function draftReducer(atom: DraftAtom, action: DraftAction): DraftAtom {
  switch (action.type) {
    case 'set_draft':
      if (atom.draft === action.text) return atom;
      return { ...atom, draft: action.text };

    case 'set_draft_if_empty':
      if (atom.draft.trim() || atom.draft === action.text) return atom;
      return { ...atom, draft: action.text };

    case 'append_draft': {
      if (!action.text) return atom;
      // Read-modify-write inside the reducer so concurrent appends
      // (terminal selection + prose-reader notes in quick succession)
      // compose deterministically.
      const next = atom.draft !== ''
        ? atom.draft + '\n\n' + action.text
        : action.text;
      return { ...atom, draft: next };
    }

    case 'clear_draft':
      if (atom.draft === '') return atom;
      return { ...atom, draft: '' };

    case 'enqueue_fenced_send_recovery':
      return {
        ...atom,
        fencedSendRecoveries: [...atom.fencedSendRecoveries, action.recovery],
      };

    case 'shift_fenced_send_recovery':
      if (atom.fencedSendRecoveries.length === 0) return atom;
      return { ...atom, fencedSendRecoveries: atom.fencedSendRecoveries.slice(1) };
  }
}

export class DraftStore extends RoutedStore<string, DraftAtom, DraftAction> {
  constructor() {
    super(() => ({ draft: '', fencedSendRecoveries: [] }), draftReducer);
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
