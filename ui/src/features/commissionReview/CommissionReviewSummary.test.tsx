import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { CommissionReviewSummaryCard } from './CommissionReviewSummary';
import type { CommissionReviewResolvedDisplayData } from './model';

const data: CommissionReviewResolvedDisplayData = {
  kind: 'commission_review',
  status: 'partial',
  reviewStatus: 'completed_with_warnings',
  findingsStatus: 'partial',
  findingsTrust: 'partial',
  retryRecommendation: 'review_findings_first',
  stageStatus: {},
  warningsSummary: [],
  findingSummary: { total: 6, critical: 0, high: 6, medium: 0, low: 0 },
  summary: {
    target: { repoRoot: '/repo', base: 'main', head: 'feature' },
    filesChanged: 5,
    filesReviewed: 3,
    insertions: 10,
    deletions: 2,
    elapsedMs: 100,
  },
  unreviewed: Array.from({ length: 4 }, (_, index) => ({ file: `src/unreviewed-${index}.ts` })),
  findings: Array.from({ length: 6 }, (_, index) => ({
    severity: 'high',
    file: `src/file-${index}.ts`,
    title: `Finding ${index}`,
    rationale: `Rationale ${index}`,
  })),
  warnings: [],
};

describe('CommissionReviewSummaryCard disclosure', () => {
  it('truncates inline details only when the full review can be opened', () => {
    render(
      <CommissionReviewSummaryCard
        data={data}
        formatDuration={() => '100ms'}
        requestSequenceId={7}
        onOpenFullReview={() => {}}
      />,
    );

    expect(screen.queryByText('Finding 5')).not.toBeInTheDocument();
    expect(screen.getByText('+1 more findings not shown')).toBeInTheDocument();
    expect(screen.queryByText('src/unreviewed-3.ts')).not.toBeInTheDocument();
    expect(screen.getByText('+1 more files')).toBeInTheDocument();
  });

  it('owns and marks retry-guidance search text', () => {
    render(
      <CommissionReviewSummaryCard
        data={data}
        formatDuration={() => '100ms'}
        activeHighlight={{ fragmentId: 'commission-review-retry', start: 15, end: 22 }}
      />,
    );

    expect(document.querySelector('[data-fragment-id="commission-review-retry"] .viewer-find-inline-match--active')).toHaveTextContent('review');
  });

  it('formats minute-scale elapsed values exactly as the tool card', () => {
    const minuteData = {
      ...data,
      summary: { ...data.summary, elapsedMs: 60_000 },
    };
    render(<CommissionReviewSummaryCard data={minuteData} formatDuration={() => '1m'} />);
    expect(document.querySelector('[data-fragment-id="commission-review-elapsed"]')).toHaveTextContent('1m');
  });

  it('owns and marks visible metric and full-detail fields', () => {
    const { rerender } = render(
      <CommissionReviewSummaryCard
        data={data}
        formatDuration={() => '100ms'}
        activeHighlight={{ fragmentId: 'commission-review-changes', start: 8, end: 10 }}
      />,
    );
    expect(document.querySelector('[data-fragment-id="commission-review-changes"] .viewer-find-inline-match--active')).toHaveTextContent('+1');

    rerender(
      <CommissionReviewSummaryCard
        data={data}
        formatDuration={() => '100ms'}
        activeHighlight={{ fragmentId: 'commission-review-status-details', start: 7, end: 14 }}
      />,
    );
    expect(document.querySelector('[data-fragment-id="commission-review-status-details"] .viewer-find-inline-match--active')).toHaveTextContent('partial');
  });

  it('renders every detail when no full review action is available', () => {
    render(<CommissionReviewSummaryCard data={data} formatDuration={() => '100ms'} />);

    expect(screen.getByText('Finding 5')).toBeInTheDocument();
    expect(screen.getByText('src/unreviewed-3.ts')).toBeInTheDocument();
    expect(screen.queryByText(/more findings not shown/)).not.toBeInTheDocument();
    expect(screen.queryByText(/more files/)).not.toBeInTheDocument();
  });
});
