import type { ChainView, ProductConversationSnapshotView } from '../../api';

export const productConversationScenarioDefinitions = [
  {
    id: 'desktop-open-multi-segment-qa-work',
    title: 'Desktop open product conversation / multi-segment + Q&A + work metadata / presentation-only',
    viewport: 'desktop',
    state: 'ready',
  },
  {
    id: 'mobile-open',
    title: 'Mobile open product conversation / compact stacked layout / presentation-only',
    viewport: 'mobile',
    state: 'ready',
  },
  {
    id: 'desktop-history-read-only',
    title: 'Desktop history product conversation / read-only handoff history',
    viewport: 'desktop',
    state: 'ready',
  },
  {
    id: 'mobile-history-read-only',
    title: 'Mobile history product conversation / read-only handoff history',
    viewport: 'mobile',
    state: 'ready',
  },
  {
    id: 'loading',
    title: 'Loading skeleton',
    viewport: 'desktop',
    state: 'loading',
  },
  {
    id: 'error',
    title: 'Initial snapshot load error',
    viewport: 'desktop',
    state: 'error',
  },
  {
    id: 'long-history-110-messages',
    title: 'Long transcript / 110+ messages across multiple segments',
    viewport: 'desktop',
    state: 'ready',
  },
] as const satisfies readonly {
  id: string;
  title: string;
  viewport: 'desktop' | 'mobile';
  state: 'ready' | 'loading' | 'error';
}[];

export type ProductConversationScenarioId = (typeof productConversationScenarioDefinitions)[number]['id'];

export interface ProductConversationScenario {
  id: ProductConversationScenarioId;
  title: string;
  viewport: 'desktop' | 'mobile';
  state: 'ready' | 'loading' | 'error';
  snapshot?: ProductConversationSnapshotView;
  chain?: ChainView;
  snapshotError?: string;
}
