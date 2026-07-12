import type { Message } from '../../api';
import { commissionReviewScenarioDefinitions } from './types';
import type {
  CommissionReviewApprovalFixtureState,
  CommissionReviewFixtureData,
  CommissionReviewScenario,
  CommissionReviewScenarioKind,
} from './types';

function agentMessage(messageId: string, blocks: unknown[], sequenceId = 1): Message {
  return {
    message_id: messageId,
    sequence_id: sequenceId,
    conversation_id: 'fixture-commission-review',
    message_type: 'agent',
    content: blocks as Message['content'],
    display_data: null,
    created_at: '2026-01-01T00:00:00Z',
  };
}

function toolMessage(toolUseId: string, content: string, sequenceId = 2): Message {
  return {
    message_id: `tool-${toolUseId}`,
    sequence_id: sequenceId,
    conversation_id: 'fixture-commission-review',
    message_type: 'tool',
    content: { tool_use_id: toolUseId, content, is_error: false },
    display_data: null,
    created_at: '2026-01-01T00:00:01Z',
  };
}

function commissionReviewDisplayData(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    kind: 'commission_review',
    status: 'success',
    review_status: 'completed',
    findings_status: 'complete',
    findings_trust: 'complete',
    retry_recommendation: 'do_not_retry',
    finding_summary: { total: 0, critical: 0, high: 0, medium: 0, low: 0 },
    warnings_summary: [],
    summary: {
      target: {
        kind: 'committed_branch_diff',
        repo_root: '/Users/example/src/phoenix-ide/.phoenix/worktrees/task-19004-style-commission-review-ui',
        base: 'origin/main',
        head: 'task-19004-style-commission-review-ui',
        dirty: false,
      },
      files_changed: 7,
      files_reviewed: 7,
      insertions: 188,
      deletions: 42,
      elapsed_ms: 2460,
      reviewer_summary: 'No correctness issues found in the reviewed diff.',
    },
    unreviewed: [],
    findings: [],
    warnings: [],
    ...overrides,
  };
}

const baseApproval: CommissionReviewApprovalFixtureState = {
  brief: 'Commission an independent code review before merge to validate the dedicated commission review UI slice and fixture coverage.',
  focus: 'Stress the approval UX, long repository metadata, and inline result states for correctness and race-condition clues.',
  scope: {
    kind: 'committed_branch_diff',
    repo_root: '/Users/example/src/phoenix-ide/very/long/path/with/nested/worktrees/task-19004-style-commission-review-ui-and-extra-suffix-for-wrap-testing',
    base: 'origin/main-with-a-surprisingly-long-reference-name-for-wrap-testing',
    head: 'task-19004-style-commission-review-ui-with-even-longer-head-branch-name',
    dirty: false,
    changed_files: 7,
    insertions: 188,
    deletions: 42,
  },
};

function inlineFixture(kind: CommissionReviewScenarioKind): CommissionReviewFixtureData['inline'] {
  const toolUseId = `tool-${kind}`;
  const message = agentMessage(`agent-${kind}`, [{
    type: 'tool_use',
    id: toolUseId,
    name: 'commission_review',
    input: {
      brief: 'Commission a merge-blocking review for the commission review presentation layer.',
      focus: 'Timer smells, event ordering, mobile overflow, and fallback containment',
    },
  }]);

  if (kind === 'inline-running') {
    return { message, toolResults: new Map(), activeToolUseId: toolUseId };
  }

  if (kind === 'inline-clean') {
    const result = toolMessage(toolUseId, JSON.stringify({ ok: true }));
    result.display_data = commissionReviewDisplayData();
    return { message, toolResults: new Map([[toolUseId, result]]) };
  }

  if (kind === 'inline-findings') {
    const result = toolMessage(toolUseId, JSON.stringify({ ok: true }));
    result.display_data = commissionReviewDisplayData({
      status: 'success',
      finding_summary: { total: 4, critical: 1, high: 1, medium: 1, low: 1 },
      summary: {
        target: {
          kind: 'committed_branch_diff',
          repo_root: '/Users/example/src/phoenix-ide/.phoenix/worktrees/task-19004-style-commission-review-ui',
          base: 'origin/main',
          head: 'task-19004-style-commission-review-ui',
          dirty: false,
        },
        files_changed: 7,
        files_reviewed: 7,
        insertions: 188,
        deletions: 42,
        elapsed_ms: 5820,
        reviewer_summary: 'Multiple correctness and resilience issues need follow-up before merge.',
      },
      findings: [
        {
          severity: 'critical', confidence: 'high', file: 'ui/src/fixtures/commissionReview/renderFixture.tsx', line: 48, symbol: 'CommissionReviewFixture',
          title: 'Ready signal can fire before required DOM is present', rationale: 'A premature ready marker would let screenshot capture race ahead of the final render.', suggested_fix: 'Derive readiness from the settled DOM rather than timing assumptions.',
        },
        {
          severity: 'high', confidence: 'high', file: 'ui/src/stories/commission-review.stories.tsx', line: 9, symbol: 'storyFor',
          title: 'Scenario routing lacks explicit unknown-id guardrails', rationale: 'Future story drift could silently capture the wrong fixture if IDs diverge.', suggested_fix: 'Resolve scenarios through a typed lookup helper.',
        },
        {
          severity: 'medium', confidence: 'medium', file: 'ui/scripts/capture-commission-review.mjs', line: 4, symbol: 'runSurfaceCapture',
          title: 'Viewport matrix omits mobile in baseline capture', rationale: 'Desktop-only capture can miss wrapping regressions in the sticky action bar.', suggested_fix: 'Capture both desktop and mobile viewports.',
        },
        {
          severity: 'low', confidence: 'medium', file: 'ui/src/components/CommissionReviewApproval.tsx', line: 154, symbol: 'Approval consequences',
          title: 'Copy could clarify token-spend consequences', rationale: 'Decision surfaces benefit from more explicit secondary copy when spend is irreversible.', suggested_fix: 'Make the review-cost consequence more prominent.',
        },
      ],
    });
    return { message, toolResults: new Map([[toolUseId, result]]) };
  }

  if (kind === 'inline-partial') {
    const result = toolMessage(toolUseId, JSON.stringify({ ok: true }));
    result.display_data = commissionReviewDisplayData({
      status: 'partial',
      review_status: 'completed_with_warnings',
      findings_status: 'partial',
      findings_trust: 'partial',
      retry_recommendation: 'review_findings_first',
      finding_summary: { total: 6, critical: 1, high: 2, medium: 2, low: 1 },
      warnings_summary: ['model output repaired after truncation', 'review ended before every changed file could be analyzed'],
      summary: {
        target: {
          kind: 'committed_branch_diff',
          repo_root: '/Users/example/src/phoenix-ide/.phoenix/worktrees/task-19004-style-commission-review-ui',
          base: 'origin/main',
          head: 'task-19004-style-commission-review-ui',
          dirty: false,
        },
        files_changed: 11,
        files_reviewed: 8,
        insertions: 402,
        deletions: 119,
        elapsed_ms: 9140,
        reviewer_summary: 'Several risks were found before coverage stopped; unreviewed files still need human attention.',
      },
      unreviewed: [
        { file: 'ui/src/pages/ConversationPage.tsx', reason: 'per_file_cap' },
        { file: 'ui/src/fixtures/commissionReview/scenarios.ts', reason: 'total_review_cap' },
        { file: 'ui/src/fixtures/commissionReview/renderFixture.tsx', reason: 'per_file_cap' },
        { file: 'ui/src/fixtures/commissionReview/renderFixture.test.tsx', reason: 'per_file_cap' },
      ],
      findings: Array.from({ length: 6 }, (_, index) => ({
        severity: index === 0 ? 'critical' : index < 3 ? 'high' : index < 5 ? 'medium' : 'low',
        confidence: 'high',
        file: `ui/src/path/finding-${index}.ts`,
        line: index + 10,
        symbol: `fn${index}`,
        title: `Finding ${index}`,
        rationale: `Rationale ${index}`,
        suggested_fix: `Fix ${index}`,
      })),
    });
    return { message, toolResults: new Map([[toolUseId, result]]) };
  }

  if (kind === 'inline-failed') {
    const result = toolMessage(toolUseId, JSON.stringify({ ok: true }));
    result.display_data = commissionReviewDisplayData({
      status: 'failed',
      review_status: 'model_timeout_no_findings',
      findings_status: 'unavailable',
      findings_trust: 'low',
      retry_recommendation: 'retry',
      warnings_summary: ['review request timed out before actionable findings were produced'],
      summary: {
        target: {
          kind: 'committed_branch_diff',
          repo_root: '/Users/example/src/phoenix-ide/.phoenix/worktrees/task-19004-style-commission-review-ui',
          base: 'origin/main',
          head: 'task-19004-style-commission-review-ui',
          dirty: false,
        },
        files_changed: 7,
        files_reviewed: 7,
        insertions: 188,
        deletions: 42,
        elapsed_ms: 12000,
        reviewer_summary: 'The model timed out before producing actionable review output.',
      },
    });
    return { message, toolResults: new Map([[toolUseId, result]]) };
  }

  const result = toolMessage(toolUseId, JSON.stringify({ ok: true }));
  result.display_data = commissionReviewDisplayData({
    status: 'rejected',
    review_status: 'rejected',
    findings_status: 'unavailable',
    findings_trust: 'low',
    summary: {
      target: {
        kind: 'committed_branch_diff',
        repo_root: '/Users/example/src/phoenix-ide/.phoenix/worktrees/task-19004-style-commission-review-ui',
        base: 'origin/main',
        head: 'task-19004-style-commission-review-ui',
        dirty: false,
      },
      files_changed: 7,
      files_reviewed: 7,
      insertions: 188,
      deletions: 42,
      elapsed_ms: 1,
      reviewer_summary: 'The review request was rejected before spending tokens.',
    },
  });
  return { message, toolResults: new Map([[toolUseId, result]]) };
}

export const commissionReviewScenarios: CommissionReviewScenario[] = commissionReviewScenarioDefinitions.map((scenario) => ({
  ...scenario,
}));

export function getCommissionReviewScenario(id: string | null | undefined): CommissionReviewScenario {
  return commissionReviewScenarios.find((scenario) => scenario.id === id) ?? commissionReviewScenarios[0]!;
}

export function commissionReviewFixtureData(scenario: CommissionReviewScenario): CommissionReviewFixtureData {
  const approval = scenario.kind === 'approval-missing-optional'
    ? {
        brief: 'Confirm whether we should spend commission tokens for a review before merge.',
        focus: null,
        scope: undefined,
      }
    : baseApproval;

  return {
    theme: scenario.theme,
    approval,
    inline: inlineFixture(scenario.kind),
  };
}
