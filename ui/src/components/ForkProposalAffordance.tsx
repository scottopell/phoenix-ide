/**
 * ForkProposalAffordance
 *
 * Inline review affordance for a decoupled task fork proposal (REQ-PROJ-034 /
 * 037). Rendered on the tool output whose `display_data.fork_proposal_id`
 * anchors it. The proposal's current status is cross-referenced from the
 * per-conversation ForkProposals store:
 *
 *  - `pending`   → a "Review" button that opens the full-screen review modal.
 *  - `spawned`   → terminal "✓ Forked" status, linking the fork conversation.
 *  - `dismissed` → terminal "✗ Dismissed" status.
 *  - `promoted`  → terminal "→ Promoted to refinement" status, linking it.
 *
 * Review is offered only while `pending` AND the origin conversation is not
 * terminal: a terminal origin can never spawn/promote (the backend will 409 and
 * retire the proposal), so a still-`pending` proposal whose origin is terminal
 * is withdrawn to a "No longer available" status rather than offering an action
 * that would fail. An already-resolved proposal keeps its real terminal status.
 *
 * Inline symbols + color follow the AGENTS.md feedback patterns (green ✓ /
 * yellow + / red ✗); no heavy panel.
 */

import { useCallback, useEffect, useRef } from 'react';
import { useNavigate } from 'react-router-dom';
import { ClipboardCheck, Check, X, ArrowRight, Loader2 } from 'lucide-react';
import { api } from '../api';
import { useForkProposals } from '../contexts/ForkProposalsContext';

interface ForkProposalAffordanceProps {
  proposalId: string;
}

export function ForkProposalAffordance({ proposalId }: ForkProposalAffordanceProps) {
  const fork = useForkProposals();
  const navigate = useNavigate();

  // Resolve a fork/refinement conversation id to its slug and navigate.
  const goToConversation = useCallback(
    async (conversationId: string | undefined) => {
      if (!conversationId) return;
      try {
        const slug = await api.getConversationSlug(conversationId);
        if (slug) navigate(`/c/${slug}`);
      } catch {
        // Resolution failure: leave the user where they are rather than
        // navigating to a broken route.
      }
    },
    [navigate],
  );

  // Ids we've already asked the store to refetch for, so a proposal that's
  // still missing after a refetch doesn't drive an infinite refetch loop.
  const requestedRef = useRef<Set<string>>(new Set());

  const proposal = fork?.getProposal(proposalId);
  const loaded = fork?.loaded ?? false;
  const refetch = fork?.refetch;

  // A proposal created after the initial list fetch (live conversation) isn't
  // in the store yet. Once the list has loaded and the id is still missing,
  // refetch once to pull in the just-streamed proposal so its Review affordance
  // appears without a page reload.
  useEffect(() => {
    if (!refetch || proposal || !loaded) return;
    if (requestedRef.current.has(proposalId)) return;
    requestedRef.current.add(proposalId);
    refetch();
  }, [refetch, proposal, loaded, proposalId]);

  if (!fork) return null;

  // List not loaded yet, or the proposal isn't in this conversation's set.
  if (!proposal) {
    // Either the first list fetch is still in flight, or we've triggered a
    // refetch for a newly-streamed id and are waiting on it. Both read as
    // "loading" so the affordance doesn't flash empty before the proposal lands.
    if (!fork.loaded || requestedRef.current.has(proposalId)) {
      return (
        <div className="fork-proposal-affordance fork-proposal-affordance--loading">
          <Loader2 size={13} className="spinning" />
          <span>Loading proposal…</span>
        </div>
      );
    }
    return null;
  }

  if (proposal.status === 'pending') {
    // A terminal origin can never spawn/promote — the backend retires this
    // proposal to `dismissed`, and approve/request-changes would 409. Withdraw
    // the Review action regardless of the (possibly stale) `pending` status and
    // show a muted withdrawn state, consistent with the terminal-status styling.
    if (fork.originTerminal) {
      return (
        <div className="fork-proposal-affordance fork-proposal-affordance--withdrawn">
          <X size={13} className="fork-proposal-affordance__icon--muted" />
          <span>No longer available</span>
        </div>
      );
    }
    return (
      <div className="fork-proposal-affordance fork-proposal-affordance--pending">
        <ClipboardCheck size={13} />
        <span className="fork-proposal-affordance__label">Task fork proposed</span>
        <button
          type="button"
          className="fork-proposal-affordance__review-btn"
          onClick={() => fork.openReview(proposalId)}
        >
          Review
        </button>
      </div>
    );
  }

  // Resolved: withdraw the Review action, show a terminal inline status.
  if (proposal.status === 'spawned') {
    return (
      <div className="fork-proposal-affordance fork-proposal-affordance--spawned">
        <Check size={13} className="fork-proposal-affordance__icon--ok" />
        <button
          type="button"
          className="fork-proposal-affordance__link"
          onClick={() => goToConversation(proposal.fork_conversation_id)}
          disabled={!proposal.fork_conversation_id}
        >
          Forked
        </button>
      </div>
    );
  }

  if (proposal.status === 'promoted') {
    return (
      <div className="fork-proposal-affordance fork-proposal-affordance--promoted">
        <ArrowRight size={13} className="fork-proposal-affordance__icon--pending" />
        <button
          type="button"
          className="fork-proposal-affordance__link"
          onClick={() => goToConversation(proposal.refinement_conversation_id)}
          disabled={!proposal.refinement_conversation_id}
        >
          Promoted to refinement
        </button>
      </div>
    );
  }

  // dismissed
  return (
    <div className="fork-proposal-affordance fork-proposal-affordance--dismissed">
      <X size={13} className="fork-proposal-affordance__icon--err" />
      <span>Dismissed</span>
    </div>
  );
}
