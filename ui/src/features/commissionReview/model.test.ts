import { describe, expect, it } from 'vitest';
import {
  commissionReviewOutcomeLabel,
  formatCommissionReviewInput,
  parseCommissionReviewDisplayData,
  parseCommissionReviewInput,
} from './model';

function displayData(overrides: Record<string, unknown> = {}) {
  return {
    kind: 'commission_review',
    status: 'partial',
    review_status: 'completed_with_warnings',
    findings_status: 'partial',
    findings_trust: 'partial',
    retry_recommendation: 'review_findings_first',
    finding_summary: { total: 1, critical: 1, high: 0, medium: 0, low: 0 },
    warnings_summary: ['model output repaired'],
    summary: {
      target: { repo_root: '/repo', base: 'origin/main', head: 'feature', dirty: false },
      files_changed: 4,
      files_reviewed: 3,
      insertions: 10,
      deletions: 2,
      elapsed_ms: 900,
      reviewer_summary: 'Summary',
    },
    unreviewed: [{ file: 'src/a.ts', reason: 'per_file_cap' }],
    findings: [{ severity: 'critical', file: 'src/a.ts', title: 'Bug', rationale: 'Why' }],
    warnings: [{ kind: 'repair', message: 'Fixed malformed output', file: 'src/a.ts' }],
    ...overrides,
  };
}

describe('commission review model', () => {
  it('formats commission review input for inline display', () => {
    expect(formatCommissionReviewInput({ brief: 'Check merge', focus: 'Correctness' })).toEqual({
      display: 'brief: Check merge\nfocus: Correctness',
      isMultiline: true,
    });
  });

  it('parses valid input and display data', () => {
    expect(parseCommissionReviewInput({ brief: 'Check merge', focus: 'Correctness' })).toEqual({
      brief: 'Check merge',
      focus: 'Correctness',
    });

    const parsed = parseCommissionReviewDisplayData(displayData());
    expect(parsed?.summary.target.repoRoot).toBe('/repo');
    expect(parsed?.warnings[0]?.message).toBe('Fixed malformed output');
    expect(parsed?.unreviewed[0]?.reason).toBe('per_file_cap');
    expect(commissionReviewOutcomeLabel(parsed!)).toBe('Partial');
  });

  it('rejects malformed display data', () => {
    expect(parseCommissionReviewDisplayData({ kind: 'commission_review', status: 'failed' })).toBeNull();
  });
});
