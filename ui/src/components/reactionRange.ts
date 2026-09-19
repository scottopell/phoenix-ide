import type { ReactionSource } from '../conversation/InlineReactionStore';

const NON_PROSE = 'button, input, textarea, [aria-hidden="true"]';

export function reactionTextOffset(block: Element, node: Node, offset: number): number {
  const prefix = document.createRange();
  prefix.selectNodeContents(block);
  prefix.setEnd(node, offset);
  const content = prefix.cloneContents();
  content.querySelectorAll(NON_PROSE).forEach((el) => el.remove());
  return content.textContent?.length ?? 0;
}

function endpoint(owner: Element, anchor: { fragmentId: string; offset: number }): { node: Node; offset: number } | null {
  const block = Array.from(owner.querySelectorAll<HTMLElement>('.agent-text-block')).find((el) =>
    el.closest<HTMLElement>('[data-fragment-id]')?.dataset['fragmentId'] === anchor.fragmentId);
  if (!block) return null;
  const walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT, {
    acceptNode: (node) => node.parentElement?.closest(NON_PROSE) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT,
  });
  let offset = 0;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const length = node.textContent?.length ?? 0;
    if (anchor.offset <= offset + length) return { node, offset: anchor.offset - offset };
    offset += length;
  }
  return null;
}

export function restoreReactionRange(source: ReactionSource, root: ParentNode = document): Range | null {
  const owner = Array.from(root.querySelectorAll<HTMLElement>('[data-inline-reaction-message]')).find((el) =>
    source.occurrenceToken ? el.dataset['messageOccurrence'] === source.occurrenceToken : el.dataset['inlineReactionMessage'] === source.messageId);
  if (!owner || !source.textAnchor) return null;
  const start = endpoint(owner, source.textAnchor.start);
  const end = endpoint(owner, source.textAnchor.end);
  if (!start || !end) return null;
  const range = document.createRange();
  range.setStart(start.node, start.offset);
  range.setEnd(end.node, end.offset);
  // Selection.toString inserts layout whitespace that Range.toString omits.
  return range.toString().replace(/\s/g, '') === source.quote.replace(/\s/g, '') ? range : null;
}
