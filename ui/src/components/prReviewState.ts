type FeedbackStatus = 'open' | 'in_progress' | 'approved' | null | undefined;

export interface PrReviewState {
  symbol: '👀' | '👍';
  label: string;
  className: string;
}

export function prReviewState(feedbackStatus: FeedbackStatus): PrReviewState | null {
  if (feedbackStatus === 'approved') {
    return {
      symbol: '👍',
      label: 'feedback approved (thumbs-up reaction)',
      className: 'pr-review-state--approved',
    };
  }
  if (feedbackStatus === 'in_progress') {
    return {
      symbol: '👀',
      label: 'feedback in progress (eyes reaction)',
      className: 'pr-review-state--in-progress',
    };
  }
  return null;
}
