import { makeVirtualizedReactionScenario } from '../fixtures/productConversation/scenarios';
import type { Story } from '@ladle/react';
import {
  getProductConversationScenario,
  makeCompactEventsScenario,
  ProductConversationFixture,
  type ProductConversationScenarioId,
} from '../fixtures/productConversation';

const storyFor = (id: ProductConversationScenarioId): Story => {
  const scenario = getProductConversationScenario(id);
  return function ProductConversationStory() {
    return <ProductConversationFixture scenario={scenario} />;
  };
};

const reactionScenario = makeVirtualizedReactionScenario();
export const InlineMessageReactions: Story = () => <ProductConversationFixture scenario={reactionScenario} />;
InlineMessageReactions.storyName = 'inline-message-reactions';

export const DesktopMultiSegmentQaWork = storyFor('desktop-multi-segment-qa-work');
DesktopMultiSegmentQaWork.storyName = 'desktop-multi-segment-qa-work';

export const MobileOpen = storyFor('mobile-open');
MobileOpen.storyName = 'mobile-open';

export const MobileContextExhausted = storyFor('mobile-context-exhausted');
MobileContextExhausted.storyName = 'mobile-context-exhausted';

export const AwaitingContinuation = storyFor('awaiting-continuation');
AwaitingContinuation.storyName = 'awaiting-continuation';

export const HistoryReadOnly = storyFor('history-read-only');
HistoryReadOnly.storyName = 'history-read-only';

export const Loading = storyFor('loading');
Loading.storyName = 'loading';

export const Error = storyFor('error');
Error.storyName = 'error';

export const LatestRowAlignedPrefixTail = storyFor('latest-row-aligned-prefix-tail');
LatestRowAlignedPrefixTail.storyName = 'latest-row-aligned-prefix-tail';

export const InputProvenanceContinuation = storyFor('input-provenance-continuation');
InputProvenanceContinuation.storyName = 'input-provenance-continuation';

export const LongHistory110Messages = storyFor('long-history-110-messages');
LongHistory110Messages.storyName = 'long-history-110-messages';

export const SourceCallGlobal = storyFor('source-call-global');
SourceCallGlobal.storyName = 'source-call-global';
export const SourceCallOrdinary = storyFor('source-call-ordinary');
SourceCallOrdinary.storyName = 'source-call-ordinary';

export const OrdinaryLazyHistory = storyFor('ordinary-lazy-history');
OrdinaryLazyHistory.storyName = 'ordinary-lazy-history';
export const GlobalLazyHistory = storyFor('global-lazy-history');
GlobalLazyHistory.storyName = 'global-lazy-history';

const compactOrdinary = makeCompactEventsScenario(false);
const compactGlobal = makeCompactEventsScenario(true);
export const CompactEventsOrdinary: Story = () => <ProductConversationFixture scenario={compactOrdinary} />;
CompactEventsOrdinary.storyName = 'compact-events-ordinary';
export const CompactEventsGlobal: Story = () => <ProductConversationFixture scenario={compactGlobal} />;
CompactEventsGlobal.storyName = 'compact-events-global';

const reminderBase = getProductConversationScenario('mobile-open');
const reminderOwner = reminderBase.snapshot?.product_conversation_id;
if (!reminderOwner) throw new Error('Reminders fixture requires a product conversation owner');
const reminderRows = [
  { id: 'due-fixture', product_conversation_id: reminderOwner, text: 'Review the stale incident report', due_at_us: 1782907200000000, timezone: 'America/Los_Angeles', revision: 1, created_at_us: 1782900000000000, state: { kind: 'due', occurred_at_us: 1782907200000000 } },
  { id: 'upcoming-fixture', product_conversation_id: reminderOwner, text: 'Check the release candidate', due_at_us: 1782993600000000, timezone: 'America/Los_Angeles', revision: 4, created_at_us: 1782900000000000, state: { kind: 'scheduled' } },
] as const;
const withReminders = (id: Extract<ProductConversationScenarioId, `reminders-${string}`>, initialFailures: number, rejectMutation = false, failuresAfterFirstLoad = 0) => ({
  ...reminderBase,
  id,
  title: `Reminders ${id.slice('reminders-'.length)}`,
  reminderFixture: { reminders: [...reminderRows], initialFailures, rejectMutation, failuresAfterFirstLoad },
});
export const RemindersInitialFailure: Story = () => <ProductConversationFixture scenario={withReminders('reminders-initial-failure', 1)} />;
RemindersInitialFailure.storyName = 'reminders-initial-failure';
export const RemindersLoadedStaleAndUpcoming: Story = () => <ProductConversationFixture scenario={withReminders('reminders-loaded-stale-upcoming', 0, false, 1)} />;
RemindersLoadedStaleAndUpcoming.storyName = 'reminders-loaded-stale-upcoming';
export const RemindersRetryAndMutationRejection: Story = () => <ProductConversationFixture scenario={withReminders('reminders-retry-mutation-rejection', 0, true, 1)} />;
RemindersRetryAndMutationRejection.storyName = 'reminders-retry-mutation-rejection';
