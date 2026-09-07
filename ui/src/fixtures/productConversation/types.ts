import type { ChainView, ProductConversationSnapshotView } from '../../api';

export const productConversationScenarioDefinitions = [
  {
    id: 'desktop-multi-segment-qa-work',
    title: 'Desktop open product conversation / continuous transcript + ordinary composer',
    viewport: 'desktop',
    state: 'ready',
  },
  {
    id: 'mobile-open',
    title: 'Mobile open product conversation / continuous transcript + ordinary composer',
    viewport: 'mobile',
    state: 'ready',
  },
  {
    id: 'mobile-context-exhausted',
    title: 'Mobile context exhausted / reachable continuation handoff',
    viewport: 'mobile',
    state: 'ready',
  },
  {
    id: 'awaiting-continuation',
    title: 'Compaction in progress / latest-row status',
    viewport: 'mobile',
    state: 'ready',
  },
  {
    id: 'history-read-only',
    title: 'History product conversation / read-only handoff history',
    viewport: 'desktop',
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
  latestConversationState?: import('../../api').ConversationState;
  /** A real cursor page returned only when the page requests snapshot.before. */
  olderSnapshot?: ProductConversationSnapshotView;
  /** Initial requests fail this many times, allowing the real Retry control to recover. */
  initialSnapshotFailures?: number;
  chain?: ChainView;
  snapshotError?: string;
}
