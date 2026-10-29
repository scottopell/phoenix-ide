import type { Story } from '@ladle/react';
import { useEffect, useState } from 'react';
import { api, type ContinueHandoffSource, type HandoffCapacityReview } from '../api';
import { ContextExhaustedHandoff } from '../components/ContextExhaustedHandoff';
import { CapacityDiagnostics } from '../pages/CapacityDiagnostics';
import type { CapacityDiagnostic } from '../generated/CapacityDiagnostic';
import '../index.css';

const original = 'Preserve the user correction: use the approved source. Finish the unresolved validation, retain evidence references, and verify completion before reporting success.';
const capacity = (ready: boolean): HandoffCapacityReview => ({
  counting_method: 'conservative_local_estimate', context_window_tokens: 32000,
  required_overhead_tokens: 3000, output_reservation_tokens: 8000,
  successor_working_room_tokens: 8192, handoff_tokens: ready ? 400 : 16000,
  provider_input_tokens: null, status: { kind: ready ? 'ready' : 'handoff_too_large', target_tokens: 12808 },
});

function RecoveryFixture({ scenario }: { scenario: 'review' | 'conflict' | 'missing-source' }) {
  const [ready, setReady] = useState(false);
  const [outcome, setOutcome] = useState('ready');
  const [reviews, setReviews] = useState(0);
  const [accepted, setAccepted] = useState<ContinueHandoffSource | null>(null);
  useEffect(() => {
    const saved = { candidates: api.handoffCandidates, review: api.reviewHandoff };
    api.handoffCandidates = async () => scenario === 'conflict' ? [{
      id: 'retained-generated-summary-with-a-long-immutable-identifier', conversation_id: 'fixture-conflict',
      origin: { kind: 'oversized_summary', operation_id: 'fixture-operation', attempt: 1 },
      source: { kind: 'unattributed_input' }, text: original,
    }] : [];
    api.reviewHandoff = async (_id, request) => {
      setReviews((value) => value + 1);
      return { source: capacity(!request.shorten), shortening: !request.shorten ? { kind: 'not_requested' }
        : outcome === 'failed' ? { kind: 'failed', error: 'Mock shortening failed; original retained.' }
          : { kind: 'generated', candidate_id: 'fixture-shortening-candidate', text: 'Keep the approved correction; finish validation and verify completion.', capacity: capacity(outcome !== 'oversized') } };
    };
    document.documentElement.dataset['theme'] = 'dark';
    setReady(true);
    return () => { api.handoffCandidates = saved.candidates; api.reviewHandoff = saved.review; };
  }, [scenario, outcome]);
  return <main style={{ width: '100%', maxWidth: 1000, margin: '0 auto', padding: 8 }} data-context-capacity-fixture-ready={ready ? scenario : undefined}>
    <h2>Recovery UI fixture · mocked responses</h2>
    <label>Shortening outcome <select value={outcome} onChange={(event) => setOutcome(event.target.value)}>
      <option value="ready">Fitting candidate</option><option value="oversized">Oversized candidate</option><option value="failed">Generation failure</option>
    </select></label>
    <p>Mock review requests: {reviews}</p>
    {ready && <ContextExhaustedHandoff parentId={`fixture-${scenario}`} generatedHandoff={scenario === 'conflict' ? '' : original}
      generatedSource={scenario === 'missing-source' || scenario === 'conflict' ? null : { kind: 'summary', message_id: 'fixture-summary-message' }}
      capacityConflict={scenario === 'conflict'} continuedInConvId={null}
      failureMessage={scenario === 'conflict' ? 'Mock deterministic capacity conflict: protected input cannot fit.' : ''}
      onOpenExisting={() => {}} onRegenerate={() => { setAccepted(null); }} onCopy={async (text) => { await navigator.clipboard.writeText(text); }}
      onContinue={async (_text, source) => { setAccepted(source); return 'accepted'; }} />}
    {accepted && <p role="status">Mock accepted source: {JSON.stringify(accepted)}</p>}
  </main>;
}

export const Review: Story = () => <RecoveryFixture scenario="review" />;
export const Conflict: Story = () => <RecoveryFixture scenario="conflict" />;
export const MissingSource: Story = () => <RecoveryFixture scenario="missing-source" />;

const observed: CapacityDiagnostic = {
  created_at: '2026-10-25T12:00:00Z', retained_input_tokens: 120, input_occupancy: 0.12,
  count_is_estimated: false, local_prediction_error_tokens: 30, provider_count_error_tokens: 0,
  attempt_status: 'observed', attempt: { outcome: 'success', provider: 'OpenAI', duration_ms: 1200, ttft_ms: 0 },
  usage: { input_tokens: 120, output_tokens: 0, cache_read_tokens: 100, cache_write_tokens: null },
  plan: {
    plan_id: 'fixture-observed', request_id: 'fixture-request', retry_attempt: 1, conversation_id: 'fixture', root_conversation_id: 'fixture',
    kind: 'ordinary', model: 'fixture-model', route_class: 'open_ai_responses_direct', connection_id: null,
    context_window_tokens: 1000, output_reservation_tokens: 200, local_prediction_tokens: 150,
    provider_input_tokens: 120, superseded_count: null, counting_method: 'open_ai_complete_input', count_fallback: null,
    reduction_action: 'clearing', reduced_local_tokens: 50, outcome: 'ready', planning_duration_ms: 10,
    prefix_comparison: 'changed', prefix_change_causes: ['clearing', 'guidance_refresh'],
  },
};
export const UsageEvidence: Story = () => <main data-context-capacity-fixture-ready="usage-evidence" style={{ maxWidth: 1000, margin: '0 auto', padding: 16 }}>
  <h2>Usage evidence fixture · mocked records</h2>
  <CapacityDiagnostics data={{ sample_limit: 200, local_refusals: 1, comparable_input_count: 1, mean_local_prediction_error_tokens: 30,
    recent: [observed, { ...observed, attempt_status: 'unknown', attempt: null, usage: null, provider_count_error_tokens: null,
      count_is_estimated: true, local_prediction_error_tokens: null, retained_input_tokens: 150, input_occupancy: 0.15,
      plan: { ...observed.plan, plan_id: 'fixture-unknown', model: 'unknown-attempt-model', provider_input_tokens: null, counting_method: 'conservative_local_estimate', count_fallback: 'timed_out' } }] }} />
</main>;
