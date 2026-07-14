import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { CommissionReviewFixture } from './renderFixture';
import { getCommissionReviewScenario } from './scenarios';

describe('CommissionReviewFixture', () => {
  it('marks approval scenarios ready and renders the real approval component', () => {
    const scenario = getCommissionReviewScenario('approval-full-dark');
    const { container } = render(<CommissionReviewFixture scenario={scenario} />);

    expect(document.documentElement.dataset['theme']).toBe('dark');
    expect(container.querySelector('[data-commission-review-fixture-ready="approval-full-dark"]')).not.toBeNull();
    expect(container.querySelector('.commission-review-approval')).not.toBeNull();
  });

  it('marks inline scenarios ready and renders the real agent message path', () => {
    const scenario = getCommissionReviewScenario('inline-partial-light');
    const { container } = render(<CommissionReviewFixture scenario={scenario} />);

    expect(document.documentElement.dataset['theme']).toBe('light');
    expect(container.querySelector('[data-commission-review-fixture-ready="inline-partial-light"]')).not.toBeNull();
    expect(container.textContent).toContain('Commission review');
    expect(container.textContent).toContain('Warnings');
  });

  it('renders full viewer stage details for commission review fixtures', () => {
    const scenario = getCommissionReviewScenario('viewer-partial-light');
    render(<CommissionReviewFixture scenario={scenario} />);

    expect(screen.getByText('target collection')).toBeInTheDocument();
    expect(screen.getByText('diff collection')).toBeInTheDocument();
    expect(screen.getByText('llm review')).toBeInTheDocument();
    expect(screen.getByText('json parse')).toBeInTheDocument();
    expect(screen.getByText('finding extraction')).toBeInTheDocument();
    expect(screen.getByText('No rationale provided by the reviewer.')).toBeInTheDocument();
  });
});
