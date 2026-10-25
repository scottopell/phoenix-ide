import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { CapacityDiagnostic } from '../generated/CapacityDiagnostic';
import { CapacityDiagnostics } from './CapacityDiagnostics';

const row: CapacityDiagnostic = {
  created_at: '2026-10-25T12:00:00Z', retained_input_tokens: 120, input_occupancy: 0.12,
  count_is_estimated: false, local_prediction_error_tokens: 30, provider_count_error_tokens: 0,
  attempt_status: 'observed', attempt: { outcome: 'success', provider: 'OpenAI', duration_ms: 1200, ttft_ms: 0 },
  usage: { input_tokens: 120, output_tokens: 0, cache_read_tokens: 100, cache_write_tokens: null },
  plan: {
    plan_id: 'preparation-a', request_id: 'request-a', retry_attempt: 0, conversation_id: 'conv', root_conversation_id: 'conv',
    kind: 'ordinary', model: 'model-a', route_class: 'open_ai_responses_direct', connection_id: null,
    context_window_tokens: 1000, output_reservation_tokens: 200, local_prediction_tokens: 150,
    provider_input_tokens: 120, superseded_count: null, counting_method: 'open_ai_complete_input', count_fallback: null,
    reduction_action: 'clearing', reduced_local_tokens: 50, outcome: 'ready', planning_duration_ms: 10,
    prefix_comparison: 'changed', prefix_change_causes: ['clearing', 'guidance_refresh', 'model_route_switch'],
  },
};
const sample = (recent: CapacityDiagnostic[]) => ({ sample_limit: 200, local_refusals: 1, comparable_input_count: 1, mean_local_prediction_error_tokens: 30, recent });
const fact = (label: string) => screen.getByText(label, { selector: 'dt' }).nextElementSibling;

describe('capacity evidence', () => {
  it('keeps the bounded sample separate from totals and complete input separate from cache usage', () => {
    render(<CapacityDiagnostics data={sample([row])} />);
    expect(screen.getByText(/Recent sample: 1 prepared requests \(up to 200\)/)).toBeInTheDocument();
    expect(screen.getByText(/not all-time totals or rates/)).toBeInTheDocument();
    fireEvent.click(screen.getByText(/model-a/));
    expect(fact('Observed complete input')).toHaveTextContent('120 tokens (includes cached input)');
    expect(fact('Observed output')).toHaveTextContent('0 tokens');
    expect(fact('Observed cache read / write')).toHaveTextContent('100 / unknown tokens');
    expect(fact('Provider TTFT')).toHaveTextContent('0 ms');
    expect(fact('Provider count − observed input')).toHaveTextContent('0 tokens');
    expect(fact('Known prefix changes')).toHaveTextContent('clearing, guidance refresh, model route switch');
    expect(screen.getByText(/Provider count · complete input/)).toBeInTheDocument();
    expect(screen.getByText(/do not establish the provider’s cache-miss cause or semantic continuity/)).toBeInTheDocument();
  });

  it('distinguishes a known local refusal from a ready request with unknown dispatch', () => {
    const refusal = { ...row, attempt_status: 'not_dispatched' as const, attempt: null, usage: null,
      count_is_estimated: true, plan: { ...row.plan, plan_id: 'refusal', outcome: 'capacity_conflict' as const, counting_method: 'conservative_local_estimate' as const, count_fallback: 'timed_out' as const } };
    const pending = { ...row, attempt_status: 'unknown' as const, attempt: null, usage: null,
      count_is_estimated: true, plan: { ...row.plan, plan_id: 'pending', counting_method: 'anthropic_estimate' as const, prefix_comparison: 'unknown' as const, prefix_change_causes: [] } };
    render(<CapacityDiagnostics data={sample([refusal, pending])} />);
    const requests = screen.getAllByRole('group');
    const [refusalRequest, pendingRequest] = requests;
    if (!refusalRequest || !pendingRequest) throw new Error('Expected both request diagnostics');
    expect(within(refusalRequest).getByText('Not dispatched')).toBeInTheDocument();
    expect(within(refusalRequest).getAllByText('not applicable')).toHaveLength(3);
    expect(within(refusalRequest).getByText('timed out')).toBeInTheDocument();
    expect(within(pendingRequest).getByText(/Attempt unknown · may be pending or interrupted/)).toBeInTheDocument();
    expect(within(pendingRequest).getByText(/Anthropic estimate/)).toBeInTheDocument();
    expect(within(pendingRequest).getByText('Observed complete input').nextElementSibling).toHaveTextContent('unknown tokens');
    expect(screen.getByText(/Conservative local estimate/)).toBeInTheDocument();
  });
});
