import assert from 'node:assert/strict';
const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i;
const marks = ['shell', 'snapshot_received', 'hydration_ready', 'meaningful_transcript', 'chapter_data_ready', 'breadcrumb_presented', 'breadcrumb_enabled', 'anchor_settled'];
export function captureDiagnostic(event) {
  assert.ok(event && event.version === 1);
  const fields = { admission: ['route', 'sampled'], mark: ['mark', 'ms'], request: ['request_id'], anchor: ['result'], terminal: ['outcome'], submission: ['state', 'status'] }[event.kind];
  assert.ok(fields && Object.keys(event).every(k => ['version', 'kind', 'open_id', ...fields].includes(k)));
  assert.ok(uuid.test(event.open_id ?? '') || (event.kind === 'admission' && event.sampled === false && event.open_id === null));
  if (event.kind === 'admission') assert.ok(['ordinary', 'global'].includes(event.route) && typeof event.sampled === 'boolean' && event.sampled === (event.open_id !== null));
  if (event.kind === 'mark') assert.ok(marks.includes(event.mark) && Number.isFinite(event.ms) && event.ms >= 0 && event.ms <= 300000);
  if (event.kind === 'request') assert.ok(uuid.test(event.request_id ?? ''));
  if (event.kind === 'anchor') assert.ok(['applied', 'tail_pinned', 'empty', 'target_missing', 'superseded'].includes(event.result));
  if (event.kind === 'terminal') assert.ok(['ready', 'failed', 'canceled', 'incomplete'].includes(event.outcome));
  if (event.kind === 'submission') assert.ok(['attempted', 'timeout_abort', 'busy_drop', 'response', 'failed'].includes(event.state) && (event.state === 'response' ? Number.isInteger(event.status) && event.status >= 100 && event.status <= 599 : event.status === null));
  return { ...event };
}
export function samplingDecision(events, route) {
  const admissions = events.filter(e => e.kind === 'admission');
  assert.equal(admissions.length, 1, 'exactly one producer admission required; missing/ambiguous evidence');
  assert.equal(admissions[0].route, route);
  assert.ok(events.every(e => e.kind === 'admission' || e.open_id === admissions[0].open_id), 'producer evidence must belong to admitted journey');
  return admissions[0];
}
export function initialFailure(trial, primary, cleanup) {
  if (primary) trial.failure = primary.message;
  if (cleanup) { trial.cleanup_failure = cleanup.message; trial.failure ??= cleanup.message; }
  return primary ?? cleanup;
}
export function requestFact(request, baseURL) {
  const url = new URL(request.url());
  if (url.origin !== new URL(baseURL).origin) return null;
  const kind = url.pathname === '/api/telemetry/conversation-view' && request.method() === 'POST' ? 'report'
    : request.method() === 'GET' && /^\/api\/product-conversations\/[a-f0-9-]{36}$/i.test(url.pathname) ? 'snapshot'
    : request.method() === 'GET' && url.pathname.startsWith('/api/global/coordinator/history/') ? 'global_history' : null;
  if (!kind) return null;
  const id = url.searchParams.get('open_id');
  return { kind, open_id: uuid.test(id ?? '') ? id : null, method: request.method(), response_status: null, request_failed: false };
}
