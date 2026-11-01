import { RoutedStore } from './RoutedStore';
import type { FileAttachment, ImageData } from '../api';

export interface RejectedSubmission {
  id: string;
  text: string;
  images: ImageData[];
  files: FileAttachment[];
  error: string;
}

const NO_REJECTIONS: readonly RejectedSubmission[] = [];

/** Snapshot identity is a revision token, including text A → B → A. */
export type DraftRevision = DraftAtom;

export interface DraftRecovery {
  readRevision(): DraftRevision;
  clearForSubmission(): DraftRevision;
  restoreIfUnchanged(revision: DraftRevision, text: string): boolean;
  retainRejected(submission: RejectedSubmission): void;
  removeRejected(id: string): void;
}

export interface ComposerDraftRecovery extends DraftRecovery {
  rejected: readonly RejectedSubmission[];
}


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
export interface DraftAtom {
  draft: string;
}

export type DraftAction =
  | { type: 'set_draft'; text: string }
  | { type: 'set_draft_if_empty'; text: string }
  | { type: 'append_draft'; text: string }
  | { type: 'clear_draft' }
  | { type: 'clear_submitted_draft' };

export function draftReducer(atom: DraftAtom, action: DraftAction): DraftAtom {
  switch (action.type) {
    case 'set_draft':
      if (atom.draft === action.text) return atom;
      return { draft: action.text };

    case 'set_draft_if_empty':
      if (atom.draft.trim() || atom.draft === action.text) return atom;
      return { draft: action.text };

    case 'append_draft': {
      if (!action.text) return atom;
      // Read-modify-write inside the reducer so concurrent appends
      // (terminal selection + prose-reader notes in quick succession)
      // compose deterministically.
      const next = atom.draft !== ''
        ? atom.draft + '\n\n' + action.text
        : action.text;
      return { draft: next };
    }

    case 'clear_submitted_draft':
      return { draft: '' };

    case 'clear_draft':
      if (atom.draft === '') return atom;
      return { draft: '' };
  }
}

export class DraftStore extends RoutedStore<string, DraftAtom, DraftAction> {
  private readonly rejected = new Map<string, readonly RejectedSubmission[]>();
  private readonly recoveries = new Map<string, DraftRecovery>();
  private readonly invalidRevision: DraftRevision = { draft: '' };

  getRejected(slug: string): readonly RejectedSubmission[] {
    return this.rejected.get(slug) ?? NO_REJECTIONS;
  }

  recovery(slug: string): DraftRecovery {
    const existing = this.recoveries.get(slug);
    if (existing) return existing;
    // Materialize the atom so removeAtom notifies recovery-only subscribers.
    this.getSnapshot(slug);
    const isCurrent = () => this.recoveries.get(slug) === recovery;
    const recovery: DraftRecovery = {
      readRevision: () => isCurrent() ? this.getSnapshot(slug) : this.invalidRevision,
      clearForSubmission: () => {
        if (!isCurrent()) return this.invalidRevision;
        this.dispatch(slug, { type: 'clear_submitted_draft' });
        return isCurrent() ? this.getSnapshot(slug) : this.invalidRevision;
      },
      restoreIfUnchanged: (revision, text) => {
        if (!isCurrent() || this.getSnapshot(slug) !== revision || revision.draft !== '') return false;
        this.dispatch(slug, { type: 'set_draft', text });
        return true;
      },
      retainRejected: submission => {
        if (!isCurrent()) return;
        this.rejected.set(slug, [...this.getRejected(slug), submission]);
        this.notifyChanged(slug);
      },
      removeRejected: id => {
        if (!isCurrent()) return;
        this.rejected.set(slug, this.getRejected(slug).filter(item => item.id !== id));
        this.notifyChanged(slug);
      },
    };
    this.recoveries.set(slug, recovery);
    return recovery;
  }

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
    this.recoveries.delete(slug);
    this.rejected.delete(slug);
    this.removeAtom(slug);
  }
}
