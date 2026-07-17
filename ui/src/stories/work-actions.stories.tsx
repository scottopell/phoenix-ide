import type { Story } from '@ladle/react';
import { WorkActionsFixture, workActionsScenarios } from '../fixtures/workActions';

const storyFor = (id: string): Story => {
  const scenario = workActionsScenarios.find((item) => item.id === id);
  if (!scenario) throw new Error(`Unknown work-actions scenario: ${id}`);
  return function WorkActionsStory() {
    return <WorkActionsFixture scenario={scenario} />;
  };
};

export const InitialPrLoading = storyFor('initial-pr-loading');
InitialPrLoading.storyName = 'initial-pr-loading';

export const CachedOpenStable = storyFor('cached-open-stable');
CachedOpenStable.storyName = 'cached-open-stable';

export const FreshAddressFeedback = storyFor('fresh-address-feedback');
FreshAddressFeedback.storyName = 'fresh-address-feedback';

export const PassingAddressFeedbackMergeSecondary = storyFor('passing-address-feedback-merge-secondary');
PassingAddressFeedbackMergeSecondary.storyName = 'passing-address-feedback-merge-secondary';

export const MergedCleanUp = storyFor('merged-clean-up');
MergedCleanUp.storyName = 'merged-clean-up';

export const NoPrDirtyReview = storyFor('no-pr-dirty-review');
NoPrDirtyReview.storyName = 'no-pr-dirty-review';

export const NoPrCreatePr = storyFor('no-pr-create-pr');
NoPrCreatePr.storyName = 'no-pr-create-pr';

export const GhUnavailable = storyFor('gh-unavailable');
GhUnavailable.storyName = 'gh-unavailable';

export const StuckOpenPr = storyFor('stuck-open-pr');
StuckOpenPr.storyName = 'stuck-open-pr';
