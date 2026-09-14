import type { Story } from '@ladle/react';
import { askUserQuestionScenarios } from '../fixtures/askUserQuestion/scenarios';
import { AskUserQuestionFixture } from '../fixtures/askUserQuestion/renderFixture';

const storyFor = (id: string): Story => {
  const scenario = askUserQuestionScenarios.find(item => item.id === id);
  if (!scenario) throw new Error(`Unknown AUQ scenario: ${id}`);
  return () => <AskUserQuestionFixture key={id} scenario={scenario} />;
};
export const Plain = storyFor('plain');
export const Preview = storyFor('preview');
export const Multiple = storyFor('multiple');
export const MultiSelect = storyFor('multi-select');
export const SubmitError = storyFor('submit-error');
export const ReadOnly = storyFor('read-only');
