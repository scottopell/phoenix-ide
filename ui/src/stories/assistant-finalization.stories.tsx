import { AssistantFinalizationFixture } from '../fixtures/assistantFinalization/renderFixture';
import '../index.css';

export const Full = () => <AssistantFinalizationFixture density="full" />;
Full.storyName = 'full';

export const Compact = () => <AssistantFinalizationFixture density="compact" />;
Compact.storyName = 'compact';
