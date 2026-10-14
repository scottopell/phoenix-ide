/** Offline only: feeds retained bytes to the installed recorder. Never starts a browser/server. */
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { installAggregateRecorder, capturePolicy, acceptedWireCursor, ownedWireCursor, REQUIRED_BODY_MAX_BYTES } from './qualify-history-opening-realserver.mjs';
const PIN = 'd4c2772a95aa46da608b356c8fe7be074de1b7aa';
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
export function readJsonLines(path) {
  const bytes = readFileSync(path); assert.ok(bytes.length <= 64 * 1024 * 1024, 'offline input file bound');
  const lines = bytes.toString('utf8').trimEnd().split('\n'); assert.ok(lines.length <= 15000, 'offline line bound');
  const fileHash = sha(bytes);
  return lines.map((line, i) => ({ value: JSON.parse(line), source: { path: resolve(path), file_sha256: fileHash, line: i + 1, line_sha256: sha(line) } }));
}
export function retainedResponse(row) {
  const r = row.value, raw = r.raw ?? r.response ?? (typeof r.body === 'string' ? r.body : undefined);
  const url = r.url ?? ('http://127.0.0.1:1' + r.path);
  assert.ok(r.truncated !== true, `truncated retained response: ${row.source.path}:${row.source.line}`);
  assert.equal(typeof raw, 'string', 'full retained body required');
  assert.ok(Buffer.byteLength(raw) <= REQUIRED_BODY_MAX_BYTES, 'retained body bound');
  if (r.body_bytes !== undefined) assert.equal(Buffer.byteLength(raw), r.body_bytes, 'retained byte count');
  return { ...row, raw, url, method: r.method ?? 'GET', status: r.status, type: r.type ?? 'application/json' };
}
export async function replayInstalled(a, rows, { openingEvidence = [], record = () => {} } = {}) {
  const page = new EventEmitter(), snapshots = [], receipts = [];
  page.mainFrame = () => page;
  const recorder = installAggregateRecorder(page, a, (file, value) => { receipts.push({ file, value }); record(file, value); }, snapshots);
  for (const evidence of openingEvidence) { recorder.registerMember(evidence.successor_transcript_row_id); recorder.registerOpening(evidence); }
  const provenance = [];
  for (const row of rows) {
    const r = retainedResponse(row);
    assert.equal(capturePolicy({ ...a, members: [...a.members, ...openingEvidence.map(e => e.successor_transcript_row_id)] }, r.method, r.url).kind, 'required_aggregate_json', 'replay only owned required responses');
    const req = { url: () => r.url, method: () => r.method, postData: () => null, failure: () => null };
    page.emit('request', req);
    page.emit('response', { request: () => req, url: () => r.url, status: () => r.status,
      headers: () => ({ 'content-type': r.type }), text: async () => r.raw });
    provenance.push({ ...row.source, replay_request_ordinal: provenance.length + 1, retained_request_ordinal: row.value.request_ordinal ?? null,
      adapter_metadata: { method: row.value.method === undefined ? 'GET inferred from retained browser response GET acquisition' : 'retained', content_type: row.value.type === undefined ? 'application/json offline adapter; not retained transport header' : 'retained', navigation_epoch: 'offline adapter zero, retained epochs remain in source row' },
      raw_sha256: sha(r.raw), body_bytes: Buffer.byteLength(r.raw), url: r.url, status: r.status });
    // Bounded sequential response adapters still invoke the exact installed async handler.
    await new Promise(r => setImmediate(r));
  }
  const drain = await recorder.sealAndDrain();
  recorder.assertQualified();
  assert.equal(snapshots.length, rows.length, 'every retained required body captured');
  return { drain, provenance, snapshots, receipts };
}
export function replayAcceptedEvents(rows, product) {
  const events = rows.map(r => r.value), cursors = [], inits = [], postAcceptedMessages = [];
  for (let i = 0; i < events.length; i++) {
    const e = events[i], d = JSON.parse(e.data);
    if (['message', 'message_updated'].includes(e.eventName) && !d.opening) {
      const initial = events.slice(0, i).findLast(candidate => candidate.eventName === 'init' && candidate.requestId === e.requestId);
      const productInits = events.filter(candidate => candidate.eventName === 'init' && JSON.parse(candidate.data).opening?.edge?.product_conversation_id === product);
      const ownedMessage = productInits.some(candidate => d.message?.message_id?.startsWith(JSON.parse(candidate.data).conversation.id + ':'));
      if (ownedMessage) assert.ok(initial && JSON.parse(initial.data).opening?.edge?.product_conversation_id === product, 'owned message must retain its Init request');
      if (initial && JSON.parse(initial.data).opening?.edge?.product_conversation_id === product) postAcceptedMessages.push({ ...rows[i].source, ...ownedWireCursor(e, events), evidence_class: 'message_after_accepted_Init_not_acceptance_transition' });
    }
    if (d.opening?.edge?.product_conversation_id !== product) continue;
    if (e.eventName === 'init') {
      assert.equal(d.conversation.id, d.opening.edge.successor_transcript_row_id);
      assert.ok(Number.isSafeInteger(d.sequence_id));
      inits.push({ ...rows[i].source, request_id: e.requestId, status: d.opening.status });
    }
    if (['message', 'message_updated'].includes(e.eventName) && d.opening.status === 'accepted') cursors.push({ ...rows[i].source, ...acceptedWireCursor(e, events) });
  }
  assert.ok(inits.length > 0 && (cursors.length > 0 || postAcceptedMessages.length > 0), 'authentic Init and message cursor required');
  return { inits, cursors, postAcceptedMessages, acceptance_transition_observed: cursors.length > 0 };
}
async function main() {
  const args = process.argv.slice(2), options = { responses: [] };
  for (let i = 0; i < args.length; i += 2) {
    assert.ok(args[i].startsWith('--') && args[i + 1], 'usage: --fixture PATH --kind ordinary|global --responses PATH [repeat] --identity PATH --source-root PATH --sse PATH [--ipc PATH] --output NEW_DIR');
    const key = args[i].slice(2); assert.ok(['fixture', 'kind', 'responses', 'identity', 'source-root', 'sse', 'ipc', 'output'].includes(key), 'unknown option');
    if (key === 'responses') options.responses.push(args[i + 1]); else { assert.equal(options[key], undefined, 'duplicate option'); options[key] = args[i + 1]; }
  }
  for (const key of ['fixture', 'kind', 'identity', 'source-root', 'sse', 'output']) assert.ok(options[key], `missing ${key}`);
  assert.ok(['ordinary', 'global'].includes(options.kind) && options.responses.length > 0);
  const identity = JSON.parse(readFileSync(options.identity)), source = resolve(options['source-root']);
  const git = (...args) => execFileSync('git', ['-C', source, ...args], { encoding: 'utf8', maxBuffer: 2 * 1024 * 1024 }).trim();
  assert.equal(identity.app_source ?? identity.source_sha, PIN, 'retained source identity');
  assert.equal(git('rev-parse', 'HEAD'), PIN); assert.equal(git('rev-parse', 'HEAD^{tree}'), identity.app_tree);
  assert.equal(git('status', '--porcelain', '--untracked-files=no'), '', 'source checkout tracked clean');
  const dtoNames = ['ProductConversationSnapshotView', 'ProductConversationHistorySnapshotView', 'CoordinatorConversationSnapshotView', 'ProductConversationSegmentView', 'ConversationHistorySegmentView', 'ProductConversationHandoffView', 'ContinuationOpeningView', 'ContinuationOpeningEdgeView'];
  const dtos = Object.fromEntries(dtoNames.map(name => { const path = `${source}/ui/src/generated/${name}.ts`; return [path, sha(readFileSync(path))]; }));
  const fixture = JSON.parse(readFileSync(options.fixture));
  const original = fixture.aggregates ? fixture.aggregates.find(a => a.kind === options.kind) : fixture;
  assert.equal(original.kind, options.kind);
  const a = structuredClone(original), transformations = [];
  const all = options.responses.flatMap(readJsonLines);
  const ipcRows = options.ipc ? readJsonLines(options.ipc) : [];
  const openingEvidence = ipcRows.filter(r => r.value.reply?.evidence || r.value.reply?.result?.evidence).map(r => r.value.reply.evidence ?? r.value.reply.result.evidence);
  const unique = [...new Map(openingEvidence.map(e => [e.successor_transcript_row_id, e])).values()];
  const prefix = a.kind === 'global' ? '/api/global/coordinator/history/' : '/api/product-conversations/';
  // Inspect full responses only in this API family. Foreign owners at owned paths still reach the installed validator.
  const family = all.filter(row => {
    const r = row.value, url = r.url ?? ('http://127.0.0.1:1' + r.path);
    return new URL(url).pathname.startsWith(prefix) && (r.method ?? 'GET') === 'GET'
      && (r.outcome === undefined || r.outcome === 'body_received');
  });
  for (const row of family) retainedResponse(row);
  if (!a.manual_opening) {
    const row = family.find(row => { const data = JSON.parse(retainedResponse(row).raw); return data.canonical_root?.transcript_row_id === a.members[0]
      && data.latest_transcript_row_id === a.members.at(-1) && data.latest_opening?.status === 'accepted'; });
    assert.ok(row, 'actual accepted aggregate opening needed');
    a.manual_opening = JSON.parse(retainedResponse(row).raw).latest_opening;
    transformations.push({ field: 'manual_opening', operation: 'append actual accepted aggregate fact to replay copy', ...row.source,
      raw_sha256: sha(retainedResponse(row).raw), value: a.manual_opening, evidence_class: 'retained_shipping_fact_not_instrumented_Global_fixture' });
  }
  a.product_conversation_id ??= a.manual_opening.edge.product_conversation_id;
  // Shipping replay can include a later accepted successor. Derive it only from a full authentic accepted aggregate.
  for (const row of family) {
    const data = JSON.parse(retainedResponse(row).raw), opening = data.latest_opening;
    if (data.canonical_root?.transcript_row_id !== a.members[0] || data.product_conversation_id !== a.product_conversation_id) continue;
    const ids = data.segments?.map(s => s.transcript_row_id);
    if (ids?.length === a.members.length + unique.length + 1 && opening?.status === 'accepted') {
      const known = [...a.members, ...unique.map(e => e.successor_transcript_row_id)];
      assert.deepEqual(ids.slice(0, -1), known, 'retained successor roster prefix');
      assert.equal(opening.edge.predecessor_transcript_row_id, known.at(-1));
      assert.equal(opening.edge.successor_transcript_row_id, ids.at(-1));
      const evidence = { ...opening.edge, client_turn_key: opening.accepted_message_id.slice(ids.at(-1).length + 1) };
      unique.push(evidence);
      transformations.push({ field: 'registered_successor_and_opening_evidence', operation: 'derive accepted edge/key from retained aggregate', ...row.source, value: evidence,
        evidence_class: 'retained_shipping_accepted_not_held_gate_evidence' });
    }
  }
  const candidates = family.filter(row => capturePolicy({ ...a, members: [...a.members, ...unique.map(e => e.successor_transcript_row_id)] }, row.value.method ?? 'GET', retainedResponse(row).url).kind === 'required_aggregate_json');
  assert.ok(candidates.length > 0 && candidates.length < 300);
  const replay = await replayInstalled(a, candidates, { openingEvidence: unique });
  const sse = replayAcceptedEvents(readJsonLines(options.sse), a.product_conversation_id);
  const output = resolve(options.output); mkdirSync(output, { recursive: false });
  const json = (file, value) => writeFileSync(`${output}/${file}`, JSON.stringify(value, null, 2) + '\n');
  json('replay-fixture.json', { ...a, replay_registered_opening_evidence: unique }); json('transformations.json', transformations); json('provenance.json', replay.provenance);
  json('installed-recorder-receipts.json', replay.receipts); json('drain.json', replay.drain); json('accepted-cursors.json', sse);
  const inputs = [options.fixture, options.identity, options.sse, ...options.responses, ...(options.ipc ? [options.ipc] : [])];
  json('manifest.json', { status: 'passed_offline_installed_recorder_replay', runtime_executed: false, kind: a.kind,
    source_sha: PIN, source_tree: identity.app_tree, source_provenance: identity.source_provenance,
    input_hashes: Object.fromEntries(inputs.map(p => [resolve(p), sha(readFileSync(p))])), generated_dto_hashes: dtos,
    recorder_sha256: sha(readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url))),
    shipping_helper_sha256: sha(readFileSync(new URL('./qualify-history-realserver.mjs', import.meta.url))),
    replay_sha256: sha(readFileSync(new URL(import.meta.url))), captured: replay.snapshots.length, sse,
    output_hashes: Object.fromEntries(['replay-fixture.json', 'transformations.json', 'provenance.json', 'installed-recorder-receipts.json', 'drain.json', 'accepted-cursors.json'].map(f => [f, sha(readFileSync(`${output}/${f}`))])),
    residual_gates: ['No held-gate/browser/physical anchor/fault/drop/reconnect runtime qualification from offline replay.', 'Global instrumented runtime remains unexecuted.'] });
  console.log(JSON.stringify({ output, captured: replay.snapshots.length, inits: sse.inits.length, accepted_cursors: sse.cursors.length }));
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) await main();
