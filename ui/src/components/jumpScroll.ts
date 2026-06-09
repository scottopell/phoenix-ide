export const JUMP_TOP_PADDING_PX = 8;

function rectFor(selector: string): DOMRect | null {
  const el = document.querySelector<HTMLElement>(selector);
  if (!el) return null;
  const rect = el.getBoundingClientRect();
  if (rect.height <= 0 || rect.width <= 0) return null;
  return rect;
}

function topEdgeOcclusionBottom(selector: string, scrollerTop: number): number | null {
  const rect = rectFor(selector);
  if (!rect || rect.top > scrollerTop) return null;
  return rect.bottom;
}

export function visibleJumpTop(scroller: HTMLElement): number {
  const scrollerTop = scroller.getBoundingClientRect().top;
  const navBottom = Math.max(
    topEdgeOcclusionBottom('#conversation-nav', scrollerTop) ?? Number.NEGATIVE_INFINITY,
    topEdgeOcclusionBottom('#breadcrumb-bar', scrollerTop) ?? Number.NEGATIVE_INFINITY,
  );
  return Math.max(scrollerTop, navBottom) + JUMP_TOP_PADDING_PX;
}

export function ensureTargetTopVisible(target: Element, scroller: HTMLElement): boolean {
  const targetTop = target.getBoundingClientRect().top;
  const desiredTop = visibleJumpTop(scroller);
  if (targetTop >= desiredTop) return false;

  scroller.scrollTop -= desiredTop - targetTop;
  return true;
}

export function findMessageScroller(): HTMLElement | null {
  return document.querySelector<HTMLElement>('#messages');
}
