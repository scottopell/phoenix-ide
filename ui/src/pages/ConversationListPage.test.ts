import { describe, expect, it } from 'vitest';
import { effectiveVisibleConversationCount } from './conversationListCount';

describe('effectiveVisibleConversationCount', () => {
  it('uses cached member counts after aggregate loading fails so scroll restoration can run', () => {
    expect(effectiveVisibleConversationCount({
      showArchived: false,
      productListError: 'offline',
      productCount: 0,
      openProductCount: 0,
      archivedProductCount: 0,
      activeMemberCount: 7,
      archivedMemberCount: 3,
    })).toBe(7);
    expect(effectiveVisibleConversationCount({
      showArchived: true,
      productListError: 'offline',
      productCount: 0,
      openProductCount: 0,
      archivedProductCount: 0,
      activeMemberCount: 7,
      archivedMemberCount: 3,
    })).toBe(3);
  });

  it('uses aggregate counts when aggregate rows are available', () => {
    expect(effectiveVisibleConversationCount({
      showArchived: false,
      productListError: null,
      productCount: 4,
      openProductCount: 3,
      archivedProductCount: 1,
      activeMemberCount: 20,
      archivedMemberCount: 10,
    })).toBe(3);
  });
});
