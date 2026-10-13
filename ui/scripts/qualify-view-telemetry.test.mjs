import test from 'node:test';
import assert from 'node:assert/strict';
import { summarizeTrials, runTelemetryChecks, candidateDestination, assertRequestCorrelation } from './qualify-view-telemetry.mjs';

test('retains insufficient natural sampling as inconclusive, never overhead clearance', () => {
  const result = summarizeTrials([{ route: 'ordinary', sampled: true }]);
  assert.equal(result.strata.ordinary.status, 'inconclusive_sampling_budget');
  assert.equal(result.overhead_clearance, false);
});
test('warmup and reset metrics do not masquerade as measured cohorts', () => {
  const trials = [];
  for (const route of ['ordinary', 'global']) for (const sampled of [true, false]) {
    trials.push(...Array.from({ length: 8 }, () => ({ route, sampled })));
    trials.push({ route, sampled, warmup: true }, { route, sampled, metric_reset: true });
  }
  const result = summarizeTrials(trials);
  assert.equal(result.strata.global.sampled, 8);
  assert.equal(result.strata.ordinary.unsampled, 8);
  assert.equal(result.strata.global.status, 'raw_cohorts_ready_for_review');
  assert.equal(result.overhead_clearance, false);
});
test('rejects nonloopback target before browser actions', async () => {
  await assert.rejects(runTelemetryChecks({ baseURL: 'https://production.example', runId: 'test' }));
});
test('fixture absolute destination cannot bypass loopback origin', () => {
  assert.equal(candidateDestination('/c/synthetic', 'http://127.0.0.1:8123'), 'http://127.0.0.1:8123/c/synthetic');
  for (const route of ['https://production.example/c/x', '//production.example/x', 'http://127.0.0.1:9999/x']) assert.throws(() => candidateDestination(route, 'http://127.0.0.1:8123'));
});
test('finite request identity must be present in journey correlation, not equal journey id', () => {
  const report = { open_id: 'journey', correlations: [{ kind: 'request', id: 'request' }] };
  assertRequestCorrelation(['request'], report);
  assert.throws(() => assertRequestCorrelation(['unrelated'], report));
  assert.throws(() => assertRequestCorrelation([], report));
});
