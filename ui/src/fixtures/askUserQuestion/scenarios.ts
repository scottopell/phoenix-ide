import type { UserQuestion } from '../../api';

const plain: UserQuestion = {
  header: 'Scope', question: 'Which conversations should search include?', multiSelect: false,
  options: [
    { label: 'Current conversation', description: 'Keep results within this conversation.' },
    { label: 'Include ancestors', description: 'Search source conversations as well.' },
    { label: 'Whole family', description: 'Include ancestors, descendants, and siblings.' },
  ],
};
const preview: UserQuestion = {
  ...plain,
  options: plain.options.map((option, i) => ({ ...option,
    description: `${option.description} ${'Useful reasoning is easier to recover, but broader retrieval can bring rejected ideas into a fresh conversation. '.repeat(3)}`,
    ...(i === 1 ? {} : { preview: `search:\n  scope: ${i === 0 ? 'current' : 'family'}\n  include_archived: false` }),
  })),
};
export const askUserQuestionScenarios = [
  { id: 'plain', questions: [plain] },
  { id: 'preview', questions: [preview] },
  { id: 'multiple', questions: [plain, { ...plain, header: 'Output', question: 'Which output format?', options: [{ label: 'Summary' }, { label: 'Detailed report' }] }, { ...plain, header: 'Checks', question: 'Which checks should run?', multiSelect: true }] },
  { id: 'multi-select', questions: [{ ...plain, multiSelect: true }] },
  { id: 'submit-error', questions: [plain], fail: true },
  { id: 'read-only', questions: [preview], readOnly: true },
] satisfies { id: string; questions: UserQuestion[]; fail?: boolean; readOnly?: boolean }[];

export type AskUserQuestionScenario = typeof askUserQuestionScenarios[number];
