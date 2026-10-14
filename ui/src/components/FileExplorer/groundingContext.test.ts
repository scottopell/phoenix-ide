import { describe, expect, it } from 'vitest';
import { conversationGroundingContext } from './groundingContext';
import type { Conversation } from '../../api';

describe('authoritative non-project grounding', () => {
  it('uses explicit serialized attached scope, never routing key as an execution identity', () => {
    const home = { runtime_role: 'coordinator', work_scope_id: 'owned', work_scope_key: 'coordinator' } as Conversation;
    expect(conversationGroundingContext(home)).toEqual({kind: 'coordinator_home', workScopeId: 'owned'});
    expect(conversationGroundingContext({runtime_role: 'coordinator'})).toEqual({kind:'coordinator_home',workScopeId:null});
  });
  it('handles Coordinator route before snapshot and keeps ordinary scope separate', () => {
    expect(conversationGroundingContext(null,true).kind).toBe('coordinator_home');
    expect(conversationGroundingContext({runtime_role:'user'} as Conversation)).toEqual({kind:'ordinary'});
  });
});
