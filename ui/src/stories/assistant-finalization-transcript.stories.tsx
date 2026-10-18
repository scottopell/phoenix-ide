import { AssistantFinalizationTranscriptFixture } from '../fixtures/assistantFinalization/transcriptFixture';
import '../index.css';

export const Full = () => <AssistantFinalizationTranscriptFixture density="full" />;
Full.storyName = 'full';

export const Compact = () => <AssistantFinalizationTranscriptFixture density="compact" />;
Compact.storyName = 'compact';

export const FullRetry = () => <AssistantFinalizationTranscriptFixture density="full" retry />;
FullRetry.storyName = 'full-retry';

export const CompactRetry = () => <AssistantFinalizationTranscriptFixture density="compact" retry />;
CompactRetry.storyName = 'compact-retry';

export const FullTable = () => <AssistantFinalizationTranscriptFixture density="full" table />;
FullTable.storyName = 'full-table';
