import { describe, expect, it } from 'vitest';
import { commissionReviewFixtureData, commissionReviewScenarios, getCommissionReviewScenario } from './scenarios';

const expectedIds = [
  'approval-full-dark',
  'approval-missing-optional-dark',
  'viewer-findings-dark',
  'viewer-partial-light',
  'inline-running-dark',
  'inline-clean-dark',
  'inline-findings-dark',
  'inline-partial-dark',
  'inline-failed-dark',
  'inline-rejected-dark',
  'approval-full-light',
  'approval-missing-optional-light',
  'inline-running-light',
  'inline-clean-light',
  'inline-findings-light',
  'inline-partial-light',
  'inline-failed-light',
  'inline-rejected-light',
] as const;

describe('commission review fixture scenarios', () => {
  it('declares the full approval, inline, and viewer matrix', () => {
    expect(commissionReviewScenarios.map(({ id }) => id)).toEqual(expectedIds);
    expect(new Set(expectedIds).size).toBe(expectedIds.length);
  });

  it('returns deterministic fixture data for every scenario', () => {
    for (const scenario of commissionReviewScenarios) {
      const data = commissionReviewFixtureData(scenario);
      expect(data.theme).toBe(scenario.theme);
      expect(data.approval.brief.length).toBeGreaterThan(20);
      expect(data.inline.message.conversation_id).toBe('fixture-commission-review');
      expect(data.inline.message.message_type).toBe('agent');

      if (scenario.kind === 'approval-missing-optional') {
        expect(data.approval.focus).toBeNull();
        expect(data.approval.scope).toBeUndefined();
      }

      if (scenario.kind === 'inline-running') {
        expect(data.inline.activeToolUseId).toBeDefined();
        expect(data.inline.toolResults.size).toBe(0);
      } else {
        expect(data.inline.toolResults.size).toBe(1);
      }
    }
  });

  it('falls back to the first scenario for unknown ids', () => {
    expect(getCommissionReviewScenario('missing-id').id).toBe(expectedIds[0]);
  });
});
