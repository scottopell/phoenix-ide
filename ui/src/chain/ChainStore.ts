import { RoutedStore } from '../conversation/RoutedStore';
import {
  chainReducer,
  createInitialChainAtom,
  type ChainAtom,
  type ChainAction,
} from './chainAtom';

/**
 * Per-rootConvId chain atoms.
 *
 * Specialization of {@link RoutedStore} for chain page state. The store
 * holds one atom per chain root id; switching chains routes to a
 * different atom and the previous chain's state is preserved untouched
 * for re-entry.
 *
 * Key consequence (the bug 08682 fixes): when ChainPage navigates from
 * chain A to chain B and A's outstanding `getChain` resolves after the
 * navigation, the dispatch lands in atom A. Atom B is unaffected. The
 * pre-08682 code put the resolution in component state directly, so the
 * resolution corrupted whichever chain happened to be mounted.
 */
export class ChainStore extends RoutedStore<string, ChainAtom, ChainAction> {
  constructor() {
    super(() => createInitialChainAtom(), chainReducer);
  }
}
