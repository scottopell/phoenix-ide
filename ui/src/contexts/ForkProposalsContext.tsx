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
  ownerGeneration: number;
  /** Target conversation id for `spawned` (the Work fork) / `promoted` (the
   *  Explore refinement). Absent for `dismissed` / `already_resolved`. */
  conversationId?: string;
}

interface ForkProposalsValue {
  /** Proposal keyed by id, or `undefined` while the list hasn't loaded yet
   *  (the affordance shows a muted loading state until then). */
  getProposal: (proposalId: string) => ForkProposalSummary | undefined;
  /** True once the origin conversation has reached a terminal state. A terminal
   *  origin can never spawn/promote, so a still-`pending` proposal is withdrawn
   *  in the UI regardless of its (possibly stale) stored status. */
  originTerminal: boolean;
  /** True until the first list fetch settles. */
  loaded: boolean;
  /** Re-fetch the proposal list (after an action, or when a new proposal id
   *  appears in the transcript). Resolves with the freshly-fetched list (or
   *  the prior list if the fetch failed), so a caller can inspect a proposal's
   *  reconciled status without racing React state. */
  refetch: () => Promise<ForkProposalSummary[]>;
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
  /** Monotonic route-owner generation captured when an action begins. */
  ownerGeneration: number;
  /** True once the origin conversation has reached a terminal state (merged /
   *  abandoned / context-exhausted / handed off). The backend retires pending
   *  proposals to `dismissed` on that transition; the provider refetches once
   *  when this flips true so the affordances withdraw without a reload. */
  originTerminal?: boolean | undefined;
  /** Called after an action resolves so the page can navigate / toast. */
  onOutcome?: ((outcome: ForkActionOutcome) => void) | undefined;
  /** Called when an action fails for a non-conflict reason (network, 4xx),
   *  so the page can surface an error toast. */
  onError?: ((message: string) => void) | undefined;
}

export function ForkProposalsProvider({
  children,
  conversationId,
  ownerGeneration,
  originTerminal,
  onOutcome,
  onError,
}: ForkProposalsProviderProps) {
  const [proposals, setProposals] = useState<ForkProposalSummary[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [openProposalId, setOpenProposalId] = useState<string | null>(null);

  // Latest conversation id, read inside async actions without re-binding them.
  const convIdRef = useRef<string | undefined>(conversationId);
  convIdRef.current = conversationId;
  const ownerGenerationRef = useRef(ownerGeneration);
  ownerGenerationRef.current = ownerGeneration;
  const actionStillOwned = useCallback(
    (id: string, generation: number) => convIdRef.current === id && ownerGenerationRef.current === generation,
    [],
  );

  // Latest fetched list, so a failed refetch can return prior state and a 409
  // handler can inspect a proposal's reconciled status without racing React's
  // async `setProposals`.
  const proposalsRef = useRef<ForkProposalSummary[]>(proposals);
  proposalsRef.current = proposals;

  const refetch = useCallback(async (): Promise<ForkProposalSummary[]> => {
    const id = convIdRef.current;
    if (!id) {
      setProposals([]);
      proposalsRef.current = [];
      setLoaded(true);
      return [];
    }
    try {
      const list = await api.listForkProposals(id);
      setProposals(list);
      proposalsRef.current = list;
      return list;
    } catch {
      // A failed list fetch leaves the prior state in place; the affordance
      // degrades to its last-known status rather than vanishing.
      return proposalsRef.current;
    } finally {
      setLoaded(true);
    }
  }, []);

  // Initial load + reload whenever the conversation changes. Guard against a
  // stale fetch from a previous conversation landing after this one switched.
  useEffect(() => {
    let cancelled = false;
    setProposals([]);
    setLoaded(false);
    const id = conversationId;
    if (!id) {
      setLoaded(true);
      return;
    }
    api
      .listForkProposals(id)
      .then((list) => {
        if (!cancelled) setProposals(list);
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) setLoaded(true);
      });
    return () => {
      cancelled = true;
    };
  }, [conversationId]);

  // N4: when the origin conversation goes terminal the backend retires its
  // pending proposals to `dismissed`. Refetch once on the false→true edge so
  // the affordances reflect that and the Review buttons withdraw. Tracking the
  // prior value (rather than refetching on every render while terminal) keeps
  // this to a single fetch per transition.
  //
  // The terminal `state_change` can arrive before the backend finishes retiring
  // the proposals, so the immediate refetch may still read `pending` rows. The
  // affordance is already correct (it withdraws on `originTerminal` directly),
  // but reconcile the stored status with a single delayed retry if any proposal
  // is still `pending` after the immediate refetch. One retry, not polling.
  const prevTerminalRef = useRef<boolean>(originTerminal ?? false);
  useEffect(() => {
    const now = originTerminal ?? false;
    const was = prevTerminalRef.current;
    prevTerminalRef.current = now;
    if (!now || was) return;
    let cancelled = false;
    let retry: ReturnType<typeof setTimeout> | undefined;
    refetch().then((list) => {
      if (cancelled) return;
      if (list.some((p) => p.status === 'pending')) {
        retry = setTimeout(() => refetch(), 1500);
      }
    });
    return () => {
      cancelled = true;
      if (retry) clearTimeout(retry);
    };
  }, [originTerminal, refetch]);

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

  // N5: a 409 from approve / request-changes is ambiguous. It means either the
  // proposal was already resolved in another tab, OR an actionable precondition
  // failed (e.g. the fork's task branch already exists outside this proposal's
  // deterministic worktree). Refetch and read the reconciled status to tell them
  // apart: a now-terminal status is a genuine resolution (close + announce
  // resolved); a still-`pending` status means the conflict was a precondition
  // failure, so surface its message and leave the modal open for a retry.
  const handleActionConflict = useCallback(
    async (
      proposalId: string,
      err: ConflictError,
      fallbackMessage: string,
      id: string,
      generation: number,
    ) => {
      if (!actionStillOwned(id, generation)) return;
      const list = await refetch();
      if (!actionStillOwned(id, generation)) return;
      const reconciled = list.find((p) => p.id === proposalId);
      if (!reconciled || reconciled.status !== 'pending') {
        setOpenProposalId(null);
        onOutcome?.({ kind: 'already_resolved', ownerGeneration: generation });
        return;
      }
      onError?.(err.message || fallbackMessage);
    },
    [actionStillOwned, refetch, onOutcome, onError],
  );

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
      const generation = ownerGenerationRef.current;
      if (!id) return;
      try {
        const { fork_conversation_id } = await api.approveForkProposal(id, proposalId);
        if (!actionStillOwned(id, generation)) return;
        markResolved(proposalId, {
          status: 'spawned',
          fork_conversation_id,
        });
        setOpenProposalId(null);
        onOutcome?.({ kind: 'spawned', conversationId: fork_conversation_id, ownerGeneration: generation });
        void refetch();
      } catch (e) {
        if (!actionStillOwned(id, generation)) return;
        if (e instanceof ConflictError) {
          await handleActionConflict(proposalId, e, 'Failed to approve proposal', id, generation);
        } else {
          onError?.(e instanceof Error ? e.message : 'Failed to approve proposal');
          void refetch();
        }
      }
    },
    [actionStillOwned, markResolved, onOutcome, onError, refetch, handleActionConflict],
  );

  const dismiss = useCallback(
    async (proposalId: string) => {
      const id = convIdRef.current;
      const generation = ownerGenerationRef.current;
      if (!id) return;
      try {
        const { no_op } = await api.dismissForkProposal(id, proposalId);
        if (!actionStillOwned(id, generation)) return;
        setOpenProposalId(null);
        if (no_op) {
          // Another tab already resolved (spawned/promoted) this proposal. Do
          // not force a local `dismissed` status — the refetch in `finally`
          // reconciles the store to the true status and its fork/refinement id.
          onOutcome?.({ kind: 'already_resolved', ownerGeneration: generation });
        } else {
          markResolved(proposalId, { status: 'dismissed' });
          onOutcome?.({ kind: 'dismissed', ownerGeneration: generation });
        }
      } catch (e) {
        if (!actionStillOwned(id, generation)) return;
        if (e instanceof ConflictError) {
          setOpenProposalId(null);
          onOutcome?.({ kind: 'already_resolved', ownerGeneration: generation });
        } else {
          onError?.(e instanceof Error ? e.message : 'Failed to dismiss proposal');
        }
      } finally {
        if (actionStillOwned(id, generation)) void refetch();
      }
    },
    [actionStillOwned, markResolved, onOutcome, onError, refetch],
  );

  const requestChanges = useCallback(
    async (proposalId: string, note: string) => {
      const id = convIdRef.current;
      const generation = ownerGenerationRef.current;
      if (!id) return;
      try {
        const { refinement_conversation_id } = await api.requestChangesForkProposal(
          id,
          proposalId,
          note,
        );
        if (!actionStillOwned(id, generation)) return;
        markResolved(proposalId, {
          status: 'promoted',
          refinement_conversation_id,
        });
        setOpenProposalId(null);
        onOutcome?.({ kind: 'promoted', conversationId: refinement_conversation_id, ownerGeneration: generation });
        void refetch();
      } catch (e) {
        if (!actionStillOwned(id, generation)) return;
        if (e instanceof ConflictError) {
          await handleActionConflict(proposalId, e, 'Failed to request changes', id, generation);
        } else {
          onError?.(e instanceof Error ? e.message : 'Failed to request changes');
          void refetch();
        }
      }
    },
    [actionStillOwned, markResolved, onOutcome, onError, refetch, handleActionConflict],
  );

  const value = useMemo<ForkProposalsValue>(
    () => ({
      getProposal,
      originTerminal: originTerminal ?? false,
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
      originTerminal,
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
