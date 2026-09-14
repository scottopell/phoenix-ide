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
export const LongPreview = storyFor('long-preview');
export const CompactHeaders = storyFor('compact-headers');
export const ReadOnly = storyFor('read-only');

import { ProductConversationFixture } from '../fixtures/productConversation/renderFixture';
import { productConversationScenarios } from '../fixtures/productConversation/scenarios';
import type { ProductConversationScenario } from '../fixtures/productConversation/types';
const productScenario: ProductConversationScenario = {
  ...productConversationScenarios[0]!,
  latestConversationState: {type:'awaiting_user_response', tool_use_id:'product-question', questions:askUserQuestionScenarios[1]!.questions},
};
export const ProductLayout: Story = () => <div data-ask-user-question-fixture="product-layout"><ProductConversationFixture scenario={productScenario} /></div>;
