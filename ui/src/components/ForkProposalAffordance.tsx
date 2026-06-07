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
 * Inline symbols + color follow the AGENTS.md feedback patterns (green ✓ /
 * yellow + / red ✗); no heavy panel.
 */

import { useCallback } from 'react';
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

  if (!fork) return null;

  const proposal = fork.getProposal(proposalId);

  // List not loaded yet, or the proposal isn't in this conversation's set.
  if (!proposal) {
    if (!fork.loaded) {
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
