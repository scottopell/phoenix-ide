import type { CapacityDiagnostics as Diagnostics } from '../generated/CapacityDiagnostics';
import type { InputCountingMethod } from '../generated/InputCountingMethod';
import './CapacityDiagnostics.css';

const countSources: Record<InputCountingMethod, string> = {
  conservative_local_estimate: 'Conservative local estimate',
  open_ai_complete_input: 'Provider count · complete input',
  anthropic_estimate: 'Anthropic estimate',
};
const words = (value: string) => value.replaceAll('_', ' ');
const tokens = (value: number | null | undefined) => value == null ? 'unknown' : value.toLocaleString();
const latency = (value: number | null | undefined) => value == null ? 'unknown' : `${value.toLocaleString()} ms`;

export function CapacityDiagnostics({ data }: { data: Diagnostics }) {
  return (
    <section className="capacity-diagnostics" aria-label="Request capacity">
      <h3>Request capacity</h3>
      <p className="usage-card__hint">
        Recent sample: {data.recent.length} prepared requests (up to {data.sample_limit}), newest first.
        {' '}These are sample counts, not all-time totals or rates.
      </p>
      <p className="capacity-diagnostics__stats">
        <span>Local refusals: {data.local_refusals}</span>
        <span>Comparable input observations: {data.comparable_input_count}</span>
        <span>Mean local prediction error: {tokens(data.mean_local_prediction_error_tokens)} tokens</span>
      </p>
      {data.recent.length === 0 && <p className="usage-card__hint">No capacity plans recorded in this sample.</p>}
      {data.recent.map((row) => {
        const { plan } = row;
        const dispatch = row.attempt_status === 'not_dispatched' ? 'Not dispatched'
          : row.attempt_status === 'observed' ? 'Provider attempt observed' : 'Attempt unknown · may be pending or interrupted';
        return (
          <details className="capacity-diagnostics__request" key={plan.plan_id}>
            <summary>
              <span><strong>{plan.model}</strong> · {words(plan.kind)} · {words(plan.outcome)}</span>
              <span>Input {tokens(row.retained_input_tokens)} · {countSources[plan.counting_method]}</span>
              <span>Output reserved {tokens(plan.output_reservation_tokens)} · Context {tokens(plan.context_window_tokens)}</span>
              <span>Reduction: {words(plan.reduction_action)} · {tokens(plan.reduced_local_tokens)} tokens</span>
            </summary>
            <dl className="capacity-diagnostics__facts">
              <div><dt>Attempt</dt><dd>{dispatch}</dd></div>
              <div><dt>Prepared</dt><dd>{row.created_at}</dd></div>
              <div><dt>Route / connection</dt><dd>{words(plan.route_class)} / {plan.connection_id ?? 'unknown'}</dd></div>
              <div><dt>Input occupancy</dt><dd>{row.input_occupancy == null ? 'unknown' : `${(row.input_occupancy * 100).toFixed(1)}%`}</dd></div>
              <div><dt>Count uncertainty</dt><dd>{row.count_is_estimated ? 'Estimated' : 'Provider counted'}</dd></div>
              <div><dt>Local prediction</dt><dd>{tokens(plan.local_prediction_tokens)} tokens</dd></div>
              <div><dt>Provider input count</dt><dd>{tokens(plan.provider_input_tokens)} tokens</dd></div>
              <div><dt>Counting fallback</dt><dd>{plan.count_fallback == null ? 'none' : words(plan.count_fallback)}</dd></div>
              <div><dt>Observed complete input</dt><dd>{tokens(row.usage?.input_tokens)} tokens (includes cached input)</dd></div>
              <div><dt>Observed output</dt><dd>{tokens(row.usage?.output_tokens)} tokens</dd></div>
              <div><dt>Observed cache read / write</dt><dd>{tokens(row.usage?.cache_read_tokens)} / {tokens(row.usage?.cache_write_tokens)} tokens</dd></div>
              <div><dt>Local prediction − observed input</dt><dd>{tokens(row.local_prediction_error_tokens)} tokens</dd></div>
              <div><dt>Provider count − observed input</dt><dd>{tokens(row.provider_count_error_tokens)} tokens</dd></div>
              <div><dt>Planning time</dt><dd>{latency(plan.planning_duration_ms)}</dd></div>
              <div><dt>Provider TTFT</dt><dd>{row.attempt_status === 'not_dispatched' ? 'not applicable' : latency(row.attempt?.ttft_ms)}</dd></div>
              <div><dt>Provider duration</dt><dd>{row.attempt_status === 'not_dispatched' ? 'not applicable' : latency(row.attempt?.duration_ms)}</dd></div>
              <div><dt>Provider result</dt><dd>{row.attempt ? `${row.attempt.provider} · ${row.attempt.outcome}` : row.attempt_status === 'not_dispatched' ? 'not applicable' : 'unknown'}</dd></div>
              <div><dt>Historical prefix</dt><dd>{words(plan.prefix_comparison)}</dd></div>
              <div><dt>Known prefix changes</dt><dd>{plan.prefix_change_causes.length ? plan.prefix_change_causes.map(words).join(', ') : 'none recorded'}</dd></div>
              <div><dt>Request / retry</dt><dd>{plan.request_id} / {plan.retry_attempt}</dd></div>
              <div><dt>Preparation ID</dt><dd>{plan.plan_id}</dd></div>
            </dl>
            <p className="usage-card__hint">Counting calls are separate from generation attempts. Planning time is separate from provider TTFT. Prefix changes do not establish the provider’s cache-miss cause or semantic continuity.</p>
            {plan.superseded_count && <p className="usage-card__hint">Earlier candidate only: local prediction {tokens(plan.superseded_count.local_prediction_tokens)} tokens; provider input count {tokens(plan.superseded_count.provider_input_tokens)} tokens · {countSources[plan.superseded_count.counting_method]} · fallback {plan.superseded_count.count_fallback == null ? 'none' : words(plan.superseded_count.count_fallback)}. This count does not describe the final prepared request and is not compared with observed usage.</p>}
          </details>
        );
      })}
    </section>
  );
}
