import { describe, expect, it } from 'vitest';
import { canShowCommissionReviewViewer } from './commissionReviewViewerPrecedence';

describe('commission review viewer precedence', () => {
  it('suppresses a requested viewer while commission approval is pending', () => {
    expect(canShowCommissionReviewViewer(true, true, 'awaiting_commission_review_approval')).toBe(false);
    expect(canShowCommissionReviewViewer(true, true, 'idle')).toBe(true);
  });
});
