export function canShowCommissionReviewViewer(canOpenSidepanel: boolean, viewerRequested: boolean, phaseType: string): boolean {
  return canOpenSidepanel && viewerRequested && phaseType !== 'awaiting_commission_review_approval';
}
