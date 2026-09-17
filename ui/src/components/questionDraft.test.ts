import { describe, expect, it } from 'vitest';
import { parseConversationState } from '../utils';
import type { UserQuestion } from '../api';
import { answerPayload, choose, createQuestionDraft, isAnswered } from './questionDraft';
const question: UserQuestion = {id:'q1',header:'Choice',question:'Which?',multiSelect:true,options:[{label:'A'},{label:'B'}]};
describe('AUQ draft payload', () => {
  it('preserves multiline custom/notes, sorts selected labels in display order, and ignores deselected custom text', () => {
    let draft = {...createQuestionDraft(question),other:'  custom\nanswer  ',notes:'  exact notes\n  '};
    draft=choose(choose(choose(draft,1),0),2);
    expect(answerPayload([question],[draft])).toEqual({answers:{q1:'A, B, custom\nanswer'},annotations:{q1:{notes:'  exact notes\n  '}}});
    draft=choose(draft,2,false);
    expect(answerPayload([question],[draft]).answers).toEqual({q1:'A, B'});
  });
  it('rejects an empty included custom answer even with other choices', () => {
    expect(isAnswered(question,choose(choose(createQuestionDraft(question),0),2))).toBe(false);
  });
  it('fails closed on a pending snapshot without identity', () => {
    const state = parseConversationState({type:'awaiting_user_response',questions:[question]});
    expect(state.type).toBe('error');
    expect(parseConversationState({type:'awaiting_user_response',request_id: 'q1', tool_use_id:'q1',questions:[question]})).toEqual({type:'awaiting_user_response',request_id: 'q1', tool_use_id:'q1',questions:[question]});
  });
});
