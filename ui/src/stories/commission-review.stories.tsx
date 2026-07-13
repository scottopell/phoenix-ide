import type { Story } from '@ladle/react';
import { CommissionReviewFixture, getCommissionReviewScenario, commissionReviewScenarios } from '../fixtures/commissionReview';

const storyFor = (id: (typeof commissionReviewScenarios)[number]['id']): Story => {
  const scenario = getCommissionReviewScenario(id);
  return function CommissionReviewStory() {
    return <CommissionReviewFixture scenario={scenario} />;
  };
};

export const ApprovalFullDark = storyFor('approval-full-dark');
ApprovalFullDark.storyName = 'approval-full-dark';

export const ApprovalMissingOptionalDark = storyFor('approval-missing-optional-dark');
ApprovalMissingOptionalDark.storyName = 'approval-missing-optional-dark';

export const ViewerFindingsDark = storyFor('viewer-findings-dark');
ViewerFindingsDark.storyName = 'viewer-findings-dark';

export const ViewerPartialLight = storyFor('viewer-partial-light');
ViewerPartialLight.storyName = 'viewer-partial-light';

export const InlineRunningDark = storyFor('inline-running-dark');
InlineRunningDark.storyName = 'inline-running-dark';

export const InlineCleanDark = storyFor('inline-clean-dark');
InlineCleanDark.storyName = 'inline-clean-dark';

export const InlineFindingsDark = storyFor('inline-findings-dark');
InlineFindingsDark.storyName = 'inline-findings-dark';

export const InlinePartialDark = storyFor('inline-partial-dark');
InlinePartialDark.storyName = 'inline-partial-dark';

export const InlineFailedDark = storyFor('inline-failed-dark');
InlineFailedDark.storyName = 'inline-failed-dark';

export const InlineRejectedDark = storyFor('inline-rejected-dark');
InlineRejectedDark.storyName = 'inline-rejected-dark';

export const ApprovalFullLight = storyFor('approval-full-light');
ApprovalFullLight.storyName = 'approval-full-light';

export const ApprovalMissingOptionalLight = storyFor('approval-missing-optional-light');
ApprovalMissingOptionalLight.storyName = 'approval-missing-optional-light';

export const InlineRunningLight = storyFor('inline-running-light');
InlineRunningLight.storyName = 'inline-running-light';

export const InlineCleanLight = storyFor('inline-clean-light');
InlineCleanLight.storyName = 'inline-clean-light';

export const InlineFindingsLight = storyFor('inline-findings-light');
InlineFindingsLight.storyName = 'inline-findings-light';

export const InlinePartialLight = storyFor('inline-partial-light');
InlinePartialLight.storyName = 'inline-partial-light';

export const InlineFailedLight = storyFor('inline-failed-light');
InlineFailedLight.storyName = 'inline-failed-light';

export const InlineRejectedLight = storyFor('inline-rejected-light');
InlineRejectedLight.storyName = 'inline-rejected-light';
