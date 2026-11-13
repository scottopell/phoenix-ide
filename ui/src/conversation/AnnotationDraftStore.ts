import { RoutedStore } from './RoutedStore';
import { formatInlineReaction, type ReactionSource } from './InlineReactionStore';

export interface DraftAnnotation {
  readonly id: string;
  readonly source: ReactionSource;
  readonly body: string;
  readonly submission: string | null;
}

export interface AnnotationSubmission {
  readonly id: string;
  readonly annotations: readonly DraftAnnotation[];
}

type Navigator = (source: ReactionSource, signal: AbortSignal) => boolean | Promise<boolean>;

export interface AnnotationDraft {
  getSnapshot(): readonly DraftAnnotation[];
  subscribe(listener: () => void): () => void;
  add(annotation: { source: ReactionSource; body: string }): void;
  edit(id: string, body: string): void;
  remove(id: string): void;
  beginSubmission(): AnnotationSubmission;
  finishSubmission(submission: AnnotationSubmission, accepted: boolean): void;
  registerNavigator(navigate: Navigator): () => void;
  returnToSource(source: ReactionSource, signal: AbortSignal): boolean | Promise<boolean>;
}

type Action = { type: 'replace'; annotations: readonly DraftAnnotation[] };
const EMPTY: readonly DraftAnnotation[] = [];

export class AnnotationDraftStore extends RoutedStore<string, readonly DraftAnnotation[], Action> {
  private readonly drafts = new Map<string, AnnotationDraft>();

  constructor() {
    super(() => EMPTY, (_, action) => action.annotations);
  }

  forScope(key: string): AnnotationDraft {
    const existing = this.drafts.get(key);
    if (existing) return existing;
    let navigate: Navigator | null = null;
    const current = () => this.drafts.get(key) === draft;
    const requireCurrent = () => {
      if (!current()) throw new Error('This annotation draft is no longer available.');
    };
    const update = (change: (annotations: readonly DraftAnnotation[]) => readonly DraftAnnotation[]) => {
      if (current()) this.dispatch(key, { type: 'replace', annotations: change(this.getSnapshot(key)) });
    };
    const draft: AnnotationDraft = {
      getSnapshot: () => current() ? this.getSnapshot(key) : EMPTY,
      subscribe: listener => this.subscribe(key, listener),
      add: annotation => {
        requireCurrent();
        update(items => [...items, { source: annotation.source, body: annotation.body, id: crypto.randomUUID(), submission: null }]);
      },
      edit: (id, body) => update(items => items.map(item => item.id === id && item.submission === null ? { ...item, body } : item)),
      remove: id => update(items => items.filter(item => item.id !== id || item.submission !== null)),
      beginSubmission: () => {
        requireCurrent();
        const submission = { id: crypto.randomUUID(), annotations: draft.getSnapshot().filter(item => item.submission === null) };
        const ids = new Set(submission.annotations.map(item => item.id));
        update(items => items.map(item => ids.has(item.id) ? { ...item, submission: submission.id } : item));
        return submission;
      },
      finishSubmission: (submission, accepted) => update(items => accepted
        ? items.filter(item => item.submission !== submission.id)
        : items.map(item => item.submission === submission.id ? { ...item, submission: null } : item)),
      registerNavigator: next => {
        navigate = next;
        return () => { if (navigate === next) navigate = null; };
      },
      returnToSource: (source, signal) => current() && navigate ? navigate(source, signal) : false,
    };
    this.drafts.set(key, draft);
    return draft;
  }

  remove(key: string): void {
    this.drafts.delete(key);
    this.removeAtom(key);
  }
}

export function formatAnnotatedMessage(text: string, annotations: readonly DraftAnnotation[]): string {
  return [text, ...annotations.map(formatInlineReaction)].filter(part => part !== '').join('\n\n');
}
