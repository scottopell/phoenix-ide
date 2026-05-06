import type { ChainView } from '../api';

/**
 * Live, not-yet-persisted Q&A entry. Held in the atom and merged into the
 * rendered list as if it were a row. On stream completion the entry is
 * dropped and the persisted row from the refetched ChainView takes its
 * place.
 */
export interface InflightQa {
  chainQaId: string;
  question: string;
  answer: string;
  /** True until the first token arrives — drives the skeleton vs streaming
   *  visual split (REQ-CHN-004 pre-token vs streaming). */
  preToken: boolean;
  /** When the SSE stream errors, surface the error inline and offer
   *  re-ask while still keeping the question visible. */
  error: string | null;
}

/**
 * Per-chain atom. One atom per `rootConvId`; held in `ChainStore`.
 *
 * Mirrors what was previously a flat collection of `useState` slots in
 * ChainPage. Promoting them out of component state into a routed atom
 * gives chain page the same dead-atom protection ConversationPage already
 * had: a navigation away from chain A while A's `getChain` is in flight
 * dispatches LOAD_OK against A's atom, not the now-rendered B's
 * component state. (Pre-08682, the equivalent ChainPage code wrote the
 * resolution into whichever component instance was currently mounted —
 * structurally a different mistake than ConversationPage made.)
 */
export interface ChainAtom {
  chain: ChainView | null;
  loadError: string | null;
  loading: boolean;
  /** Keyed by chain_qa_id; values include question, answer accumulator,
   *  preToken state, and any error from the chain SSE stream. */
  inflight: Record<string, InflightQa>;
  /** Submission order. Object key order on `inflight` alone is not a
   *  reliable ordering source. */
  inflightOrder: string[];
  draft: string;
  submitting: boolean;
  /** True when the EventSource has errored and there is in-flight work
   *  that depends on it. UI surfaces a "connection lost" affordance. */
  sseLost: boolean;
}

export function createInitialChainAtom(): ChainAtom {
  return {
    chain: null,
    loadError: null,
    loading: true,
    inflight: {},
    inflightOrder: [],
    draft: '',
    submitting: false,
    sseLost: false,
  };
}

export type ChainAction =
  // Load lifecycle.
  | { type: 'LOAD_BEGIN' }
  | { type: 'LOAD_OK'; view: ChainView }
  | { type: 'LOAD_FAIL'; error: string }

  // Inline name edit / archive / delete also produce LOAD_OK with the
  // returned view; no special action needed.

  // Submit lifecycle. Optimistic: OPTIMISTIC_INFLIGHT_ADD lands
  // synchronously before the POST resolves. SUBMIT_BEGIN/OK/FAIL track
  // the network round-trip independently from the in-flight buffer.
  | { type: 'SUBMIT_BEGIN' }
  | {
      type: 'OPTIMISTIC_INFLIGHT_ADD';
      chainQaId: string;
      question: string;
    }
  | { type: 'SUBMIT_OK' }
  | { type: 'SUBMIT_FAIL'; error: string }

  // Wire-driven mutations of the in-flight buffer.
  | { type: 'TOKEN_APPENDED'; chainQaId: string; delta: string }
  | {
      type: 'INFLIGHT_FAIL';
      chainQaId: string;
      error: string;
      partialAnswer: string | null;
    }
  | { type: 'INFLIGHT_DROP'; chainQaId: string }

  // Draft + UI affordances.
  | { type: 'DRAFT_CHANGED'; value: string }
  | { type: 'SSE_LOST' }
  | { type: 'SSE_RESTORED' }

  // Used by re-ask: populate the active textarea with a previous question.
  | { type: 'DRAFT_SET'; value: string };

export function chainReducer(atom: ChainAtom, action: ChainAction): ChainAtom {
  switch (action.type) {
    case 'LOAD_BEGIN': {
      // Loading flag flips on regardless of whether we already have a
      // chain snapshot. Network failure mid-load doesn't clear `chain`
      // — see LOAD_FAIL.
      if (atom.loading && atom.loadError === null) return atom;
      return { ...atom, loading: true, loadError: null };
    }

    case 'LOAD_OK': {
      return { ...atom, chain: action.view, loadError: null, loading: false };
    }

    case 'LOAD_FAIL': {
      // Preserve the existing `chain` snapshot if we have one (REQ-CHN-005:
      // history persists through transient errors). Surface the error
      // alongside.
      return { ...atom, loadError: action.error, loading: false };
    }

    case 'SUBMIT_BEGIN': {
      if (atom.submitting) return atom;
      return { ...atom, submitting: true };
    }

    case 'OPTIMISTIC_INFLIGHT_ADD': {
      // The chain_qa_id is server-issued today (returned from the POST).
      // ChainPage submits and awaits before this fires. Future improvement:
      // mint a client-only id, swap in the server id on SUBMIT_OK. For
      // now the action exists so the dispatch remains a single side-effect
      // boundary even when the id arrives from the server.
      if (atom.inflight[action.chainQaId]) return atom;
      return {
        ...atom,
        inflight: {
          ...atom.inflight,
          [action.chainQaId]: {
            chainQaId: action.chainQaId,
            question: action.question,
            answer: '',
            preToken: true,
            error: null,
          },
        },
        inflightOrder: [...atom.inflightOrder, action.chainQaId],
        draft: '',
      };
    }

    case 'SUBMIT_OK': {
      if (!atom.submitting) return atom;
      return { ...atom, submitting: false };
    }

    case 'SUBMIT_FAIL': {
      // Keep the draft so the user can retry without retyping; surface the
      // error via `loadError` (the page-level error banner). The
      // optimistic inflight entry, if any, is left in place — the user
      // sees their question; the server-side row never persisted.
      return { ...atom, submitting: false, loadError: action.error };
    }

    case 'TOKEN_APPENDED': {
      const cur = atom.inflight[action.chainQaId];
      if (!cur) return atom;
      return {
        ...atom,
        inflight: {
          ...atom.inflight,
          [action.chainQaId]: {
            ...cur,
            answer: cur.answer + action.delta,
            preToken: false,
          },
        },
      };
    }

    case 'INFLIGHT_FAIL': {
      const cur = atom.inflight[action.chainQaId];
      if (!cur) return atom;
      return {
        ...atom,
        inflight: {
          ...atom.inflight,
          [action.chainQaId]: {
            ...cur,
            answer: action.partialAnswer ?? cur.answer,
            error: action.error,
            preToken: false,
          },
        },
      };
    }

    case 'INFLIGHT_DROP': {
      if (!(action.chainQaId in atom.inflight)) return atom;
      const next = { ...atom.inflight };
      delete next[action.chainQaId];
      return {
        ...atom,
        inflight: next,
        inflightOrder: atom.inflightOrder.filter((id) => id !== action.chainQaId),
      };
    }

    case 'DRAFT_CHANGED': {
      if (atom.draft === action.value) return atom;
      return { ...atom, draft: action.value };
    }

    case 'DRAFT_SET': {
      if (atom.draft === action.value) return atom;
      return { ...atom, draft: action.value };
    }

    case 'SSE_LOST': {
      if (atom.sseLost) return atom;
      return { ...atom, sseLost: true };
    }

    case 'SSE_RESTORED': {
      if (!atom.sseLost) return atom;
      return { ...atom, sseLost: false };
    }
  }
}
