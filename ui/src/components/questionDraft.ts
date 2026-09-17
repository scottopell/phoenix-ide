import type { UserQuestion } from '../api';

export type QuestionDraft = {
  selection: { kind: 'single'; value: number | null } | { kind: 'multiple'; values: number[] };
  other: string;
  notes: string;
  notesOpen: boolean;
  previewOpen: boolean;
};
export const createQuestionDraft = (question: UserQuestion): QuestionDraft => ({
  selection: question.multiSelect ? { kind: 'multiple', values: [] } : { kind: 'single', value: null },
  other: '', notes: '', notesOpen: false, previewOpen: false,
});
const questionKey = (question: UserQuestion, index: number) => question.id ?? `q${index + 1}`;
export function selected(draft: QuestionDraft, index: number): boolean {
  return draft.selection.kind === 'single' ? draft.selection.value === index : draft.selection.values.includes(index);
}
export function choose(draft: QuestionDraft, index: number, checked = true): QuestionDraft {
  return { ...draft, selection: draft.selection.kind === 'single'
    ? { kind: 'single', value: checked ? index : null }
    : { kind: 'multiple', values: checked ? [...new Set([...draft.selection.values, index])] : draft.selection.values.filter(value => value !== index) } };
}
export function isAnswered(question: UserQuestion, draft: QuestionDraft): boolean {
  if (selected(draft, question.options.length) && !draft.other.trim()) return false;
  return draft.selection.kind === 'single' ? draft.selection.value !== null : draft.selection.values.length > 0;
}
export function answerPayload(questions: UserQuestion[], drafts: QuestionDraft[]) {
  const answers: Record<string, string> = {};
  const annotations: Record<string, { notes?: string; preview?: string }> = {};
  questions.forEach((question, index) => {
    const draft = drafts[index]!;
    const labels = question.options.filter((_, option) => selected(draft, option)).map(option => option.label);
    if (selected(draft, question.options.length)) labels.push(draft.other.trim());
    const key = questionKey(question, index);
    answers[key] = labels.join(', ');
    const preview = draft.selection.kind === 'single' && draft.selection.value !== null
      ? question.options[draft.selection.value]?.preview : undefined;
    if (draft.notes || preview) annotations[key] = {
      ...(draft.notes ? { notes: draft.notes } : {}), ...(preview ? { preview } : {}),
    };
  });
  return { answers, ...(Object.keys(annotations).length ? { annotations } : {}) };
}
