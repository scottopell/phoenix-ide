import type { ReactionSource } from '../conversation/InlineReactionStore';

export function restoreReactionRange(source: ReactionSource): Range | null {
  const owner = Array.from(document.querySelectorAll<HTMLElement>('#messages [data-inline-reaction-message]')).find((el) =>
    source.occurrenceToken ? el.dataset['messageOccurrence'] === source.occurrenceToken : el.dataset['inlineReactionMessage'] === source.messageId);
  if (!owner || !source.textOffsets) return null;
  const walker = document.createTreeWalker(owner, NodeFilter.SHOW_TEXT);
  const range = document.createRange();
  let offset = 0;
  let started = false;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const length = node.textContent?.length ?? 0;
    if (!started && source.textOffsets.start <= offset + length) {
      range.setStart(node, source.textOffsets.start - offset);
      started = true;
    }
    if (started && source.textOffsets.end <= offset + length) {
      range.setEnd(node, source.textOffsets.end - offset);
      return range;
    }
    offset += length;
  }
  return null;
}
