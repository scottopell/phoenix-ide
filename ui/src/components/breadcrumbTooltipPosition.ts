interface TooltipPosition {
  tooltipLeft: number;
  tooltipTop: number;
  arrowLeft: number;
}

const TOOLTIP_WIDTH = 280;
const TOOLTIP_MARGIN = 8;
const TOOLTIP_VERTICAL_GAP = 8;

export function calcTooltipPosition(rect: DOMRect): TooltipPosition {
  const itemCenterX = rect.left + rect.width / 2;
  const viewportWidth = window.innerWidth;

  let tooltipLeft = itemCenterX - TOOLTIP_WIDTH / 2;
  tooltipLeft = Math.max(TOOLTIP_MARGIN, tooltipLeft);
  tooltipLeft = Math.min(viewportWidth - TOOLTIP_WIDTH - TOOLTIP_MARGIN, tooltipLeft);

  const arrowLeft = Math.max(12, Math.min(TOOLTIP_WIDTH - 12, itemCenterX - tooltipLeft));

  return {
    tooltipLeft,
    tooltipTop: Math.max(TOOLTIP_MARGIN, rect.top - TOOLTIP_VERTICAL_GAP),
    arrowLeft,
  };
}
