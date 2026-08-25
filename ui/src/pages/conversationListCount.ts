export function effectiveVisibleConversationCount({
  showArchived,
  productListError,
  productCount,
  openProductCount,
  archivedProductCount,
  activeMemberCount,
  archivedMemberCount,
}: {
  showArchived: boolean;
  productListError: string | null;
  productCount: number;
  openProductCount: number;
  archivedProductCount: number;
  activeMemberCount: number;
  archivedMemberCount: number;
}): number {
  const usingCachedMembers = !!productListError && productCount === 0;
  return usingCachedMembers
    ? (showArchived ? archivedMemberCount : activeMemberCount)
    : (showArchived ? archivedProductCount : openProductCount);
}
