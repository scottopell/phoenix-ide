import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import type { ReactNode } from 'react';
import { api, ConflictError } from '../api';
import type { ForkProposalSummary } from '../api';

/**
 * Per-conversation store for decoupled task fork proposals (REQ-PROJ-034 / 037).
 *
 * The proposal id rides the existing tool-result `display_data.fork_proposal_id`
 * (the snapshot body is deliberately NOT in the transcript). A tool output that
 * carries that id renders an inline Review affordance; the affordance looks the
 * proposal up here by id to read its current `status`. Only `pending` proposals
 * are reviewable — once resolved (`spawned` / `dismissed` / `promoted`) the
 * affordance withdraws and a terminal inline status shows instead.
 *
 * SSE for proposal status is not required: the list is fetched on load and
 * refetched after any action (and when a new fork-proposal tool output appears).
 */

/** Outcome surfaced to the page after a proposal action resolves, so the page
 *  can navigate / toast (the provider owns no router or toast of its own). */
export interface ForkActionOutcome {
  kind: 'spawned' | 'dismissed' | 'promoted' | 'already_resolved';
  /** Target conversation id for `spawned` (the Work fork) / `promoted` (the
   *  Explore refinement). Absent for `dismissed` / `already_resolved`. */
  conversationId?: string;
}

interface ForkProposalsValue {
  /** Proposal keyed by id, or `undefined` while the list hasn't loaded yet
   *  (the affordance shows a muted loading state until then). */
  getProposal: (proposalId: string) => ForkProposalSummary | undefined;
  /** True until the first list fetch settles. */
  loaded: boolean;
  /** Re-fetch the proposal list (after an action, or when a new proposal id
   *  appears in the transcript). */
  refetch: () => void;
  /** The proposal whose review modal is open, if any. */
  openProposalId: string | null;
  openReview: (proposalId: string) => void;
  closeReview: () => void;
  approve: (proposalId: string) => Promise<void>;
  dismiss: (proposalId: string) => Promise<void>;
  requestChanges: (proposalId: string, note: string) => Promise<void>;
}

const ForkProposalsContext = createContext<ForkProposalsValue | null>(null);

export interface ForkProposalsProviderProps {
  children: ReactNode;
  /** Origin conversation whose proposals these are. When absent (no
   *  conversation yet) the provider is inert: it fetches nothing and every
   *  lookup misses. */
  conversationId?: string | undefined;
  /** Called after an action resolves so the page can navigate / toast. */
  onOutcome?: ((outcome: ForkActionOutcome) => void) | undefined;
  /** Called when an action fails for a non-conflict reason (network, 4xx),
   *  so the page can surface an error toast. */
  onError?: ((message: string) => void) | undefined;
}

export function ForkProposalsProvider({
  children,
  conversationId,
  onOutcome,
  onError,
}: ForkProposalsProviderProps) {
  const [proposals, setProposals] = useState<ForkProposalSummary[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [openProposalId, setOpenProposalId] = useState<string | null>(null);

  // Latest conversation id, read inside async actions without re-binding them.
  const convIdRef = useRef<string | undefined>(conversationId);
  convIdRef.current = conversationId;

  const refetch = useCallback(() => {
    const id = convIdRef.current;
    if (!id) {
      setProposals([]);
      setLoaded(true);
      return;
    }
    let cancelled = false;
    api
      .listForkProposals(id)
      .then((list) => {
        if (!cancelled) {
          setProposals(list);
          setLoaded(true);
        }
      })
      .catch(() => {
        // A failed list fetch leaves the prior state in place; the affordance
        // degrades to its last-known status rather than vanishing.
        if (!cancelled) setLoaded(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Initial load + reload whenever the conversation changes.
  useEffect(() => {
    setProposals([]);
    setLoaded(false);
    const cleanup = refetch();
    return cleanup;
  }, [conversationId, refetch]);

  const byId = useMemo(() => {
    const m = new Map<string, ForkProposalSummary>();
    for (const p of proposals) m.set(p.id, p);
    return m;
  }, [proposals]);

  const getProposal = useCallback(
    (proposalId: string) => byId.get(proposalId),
    [byId],
  );

  const openReview = useCallback((proposalId: string) => setOpenProposalId(proposalId), []);
  const closeReview = useCallback(() => setOpenProposalId(null), []);

  /** Optimistically mark a proposal resolved so the affordance withdraws
   *  immediately; the subsequent refetch reconciles with the server. */
  const markResolved = useCallback(
    (proposalId: string, patch: Partial<ForkProposalSummary>) => {
      setProposals((prev) =>
        prev.map((p) => (p.id === proposalId ? { ...p, ...patch } : p)),
      );
    },
    [],
  );

  const approve = useCallback(
    async (proposalId: string) => {
      const id = convIdRef.current;
      if (!id) return;
      try {
        const { fork_conversation_id } = await api.approveForkProposal(id, proposalId);
        markResolved(proposalId, {
          status: 'spawned',
          fork_conversation_id,
        });
        setOpenProposalId(null);
        onOutcome?.({ kind: 'spawned', conversationId: fork_conversation_id });
      } catch (e) {
        if (e instanceof ConflictError) {
          setOpenProposalId(null);
          onOutcome?.({ kind: 'already_resolved' });
        } else {
          onError?.(e instanceof Error ? e.message : 'Failed to approve proposal');
        }
      } finally {
        refetch();
      }
    },
    [markResolved, onOutcome, onError, refetch],
  );

  const dismiss = useCallback(
    async (proposalId: string) => {
      const id = convIdRef.current;
      if (!id) return;
      try {
        await api.dismissForkProposal(id, proposalId);
        markResolved(proposalId, { status: 'dismissed' });
        setOpenProposalId(null);
        onOutcome?.({ kind: 'dismissed' });
      } catch (e) {
        if (e instanceof ConflictError) {
          setOpenProposalId(null);
          onOutcome?.({ kind: 'already_resolved' });
        } else {
          onError?.(e instanceof Error ? e.message : 'Failed to dismiss proposal');
        }
      } finally {
        refetch();
      }
    },
    [markResolved, onOutcome, onError, refetch],
  );

  const requestChanges = useCallback(
    async (proposalId: string, note: string) => {
      const id = convIdRef.current;
      if (!id) return;
      try {
        const { refinement_conversation_id } = await api.requestChangesForkProposal(
          id,
          proposalId,
          note,
        );
        markResolved(proposalId, {
          status: 'promoted',
          refinement_conversation_id,
        });
        setOpenProposalId(null);
        onOutcome?.({ kind: 'promoted', conversationId: refinement_conversation_id });
      } catch (e) {
        if (e instanceof ConflictError) {
          setOpenProposalId(null);
          onOutcome?.({ kind: 'already_resolved' });
        } else {
          onError?.(e instanceof Error ? e.message : 'Failed to request changes');
        }
      } finally {
        refetch();
      }
    },
    [markResolved, onOutcome, onError, refetch],
  );

  const value = useMemo<ForkProposalsValue>(
    () => ({
      getProposal,
      loaded,
      refetch,
      openProposalId,
      openReview,
      closeReview,
      approve,
      dismiss,
      requestChanges,
    }),
    [
      getProposal,
      loaded,
      refetch,
      openProposalId,
      openReview,
      closeReview,
      approve,
      dismiss,
      requestChanges,
    ],
  );

  return (
    <ForkProposalsContext.Provider value={value}>
      {children}
    </ForkProposalsContext.Provider>
  );
}

/** Read the fork-proposals store. Returns `null` outside a provider so leaf
 *  components (e.g. a tool block rendered in a share view that has no provider)
 *  can no-op instead of crashing. */
// eslint-disable-next-line react-refresh/only-export-components
export function useForkProposals(): ForkProposalsValue | null {
  return useContext(ForkProposalsContext);
}
