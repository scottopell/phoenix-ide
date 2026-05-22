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

  const tooltipWidth = Math.min(TOOLTIP_WIDTH, Math.max(0, viewportWidth - TOOLTIP_MARGIN * 2));
  const maxTooltipLeft = Math.max(TOOLTIP_MARGIN, viewportWidth - tooltipWidth - TOOLTIP_MARGIN);

  let tooltipLeft = itemCenterX - tooltipWidth / 2;
  tooltipLeft = Math.max(TOOLTIP_MARGIN, tooltipLeft);
  tooltipLeft = Math.min(maxTooltipLeft, tooltipLeft);

  const arrowLeft = Math.max(12, Math.min(tooltipWidth - 12, itemCenterX - tooltipLeft));

  return {
    tooltipLeft,
    tooltipTop: Math.max(TOOLTIP_MARGIN, rect.top - TOOLTIP_VERTICAL_GAP),
    arrowLeft,
  };
}
