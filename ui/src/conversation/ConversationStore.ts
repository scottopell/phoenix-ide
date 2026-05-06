import { createInitialAtom, conversationReducer } from './atom';
import type { ConversationAtom, SSEAction } from './atom';
import { RoutedStore } from './RoutedStore';

/**
 * Per-slug conversation atoms.
 *
 * A thin specialization of {@link RoutedStore}: the routing, subscription,
 * and notify logic lives upstream; this class binds the key (`string` slug),
 * atom shape (`ConversationAtom`), and reducer (`conversationReducer`).
 * Using `useSyncExternalStore` with this store gives per-slug isolation:
 * a streaming token in conversation A does not cause conversation B's
 * consumer to re-render.
 */
export class ConversationStore extends RoutedStore<string, ConversationAtom, SSEAction> {
  constructor() {
    super(() => createInitialAtom(), conversationReducer);
  }
}
