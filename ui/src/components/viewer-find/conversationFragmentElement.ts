import type { ConversationFragmentRevealTarget } from './searchProjections';

export function findConversationFragmentElement(
  row: Element,
  fragmentId: string,
  revealTarget: ConversationFragmentRevealTarget,
): Element | null {
  const owner = 'toolUseId' in revealTarget
    ? row.querySelector(`[data-tool-id="${CSS.escape(revealTarget.toolUseId)}"]`)
    : row;
  return owner?.querySelector(`[data-fragment-id="${CSS.escape(fragmentId)}"]`) ?? null;
}
