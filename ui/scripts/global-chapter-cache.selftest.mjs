import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { stripTypeScriptTypes } from 'node:module';
const sha = raw => createHash('sha256').update(raw).digest('hex');
const rewriteCache = f => { f.cached.raw = JSON.stringify(f.cached.data); f.cached.body_bytes = Buffer.byteLength(f.cached.raw); f.cached.raw_sha256 = sha(f.cached.raw); };
import { validateGlobalCacheAuthority, validateGlobalChapterSelection, validateGlobalChapterReference, globalChapterReferencePath, capturePolicy } from './qualify-history-opening-realserver.mjs';
const dir = process.env.HISTORY_GLOBAL_0BAC_EVIDENCE ?? '.phoenix/evidence/continuous-history/instrumented0bac-artifact/opening-browser-evidence/global';
const lines = name => readFileSync(`${dir}/${name}`, 'utf8').trim().split('\n').map(JSON.parse);
const a = JSON.parse(readFileSync(`${dir}/fixture.json`));
const evidence = lines('ipc.jsonl').find(r => r.reply.evidence).reply.evidence;
const latest = evidence.successor_transcript_row_id, id = `${latest}:${evidence.client_turn_key}`;
const events = lines('browser-sse.jsonl');
const accepted = events.find(e => ['message', 'message_updated'].includes(e.eventName) && JSON.parse(e.data).opening?.accepted_message_id === id);
const reconnect = lines('assertions.jsonl').find(r => r.name === 'same-page transport SSE drop/reconnect preserves accepted full-edge identity').reconnect;
const cachedReceipt = lines('browser-responses.jsonl').find(r => r.raw && JSON.parse(r.raw).latest_opening?.client_turn_key === evidence.client_turn_key && JSON.parse(r.raw).segments.length === a.members.length + 1);
const cached = { ...cachedReceipt, data: JSON.parse(cachedReceipt.raw) };
const row = JSON.parse(lines('driver-http.jsonl').filter(r => r.method === 'GET' && r.path === `/api/conversations/${latest}`).at(-1).raw);
const ack = lines('assertions.jsonl').find(r => r.name === 'actual successor chat ACK receipt');
const instruction = lines('driver-http.jsonl').find(r => r.method === 'POST' && r.path.endsWith('/continue')).body.handoff;
const requests = lines('browser-requests.jsonl');
const chatRequest = JSON.parse(requests.find(r => r.method === 'POST' && new URL(r.url).pathname === `/api/conversations/${latest}/chat`).body);
const stream = requests.find(r => new URL(r.url).pathname === `/api/conversations/${latest}/stream`).url;
// Retained logs lack CDP requestWillBeSent mappings. This URL association is an OFFLINE adapter,
// not new requestId provenance; live validation uses the actual CDP request map.
const facts = () => structuredClone({ evidence, cached, accepted, reconnect, streamUrls: new Map([[accepted.requestId, stream], [reconnect.requestId, stream]]), row, ack, chatRequest, instruction });
const selection = `http://127.0.0.1:1/global/${a.members[0]}?source_transcript=${latest}#message-${encodeURIComponent(id)}`;
const physical = { count: 1, visible: true, persistedMessageID: id }; // UNIT ONLY; no retained DOM claims.
test('authentic 0bac cache/SSE/ACK bytes support authority; not final Global qualification', () => {
  assert.equal(validateGlobalCacheAuthority(a, facts()).latest, latest);
  const result = JSON.parse(readFileSync(`${dir}/browser-result.json`));
  assert.equal(result.status, 'failed'); assert.equal(result.checks.length, 9); assert.match(result.error, /waitForResponse/);
  const lastProof = lines('assertions.jsonl').at(-1);
  const cutoff = lastProof.requests.at(-1).request_ordinal;
  const slice = requests.filter(r => r.request_ordinal > cutoff);
  assert.equal(slice.filter(r => capturePolicy({ ...a, members: [...a.members, latest] }, r.method, r.url).kind === 'required_aggregate_json').length, 0);
  assert.ok(lines('browser-responses.jsonl').some(r => r.status === 404 && r.url.includes('/api/product-conversations/')));
  assert.ok(slice.length > 0, 'actual observed post-reconnect request slice, not nofetch promise');
});
for (const [name, mutate] of Object.entries({
  product: f => { f.evidence.product_conversation_id = 'foreign'; },
  root: f => { f.cached.data.canonical_root.transcript_row_id = 'foreign'; rewriteCache(f); },
  current_latest: f => { f.evidence.successor_transcript_row_id = 'foreign'; },
  full_roster: f => { f.cached.data.segments.pop(); rewriteCache(f); },
  pending_key: f => { f.evidence.client_turn_key = 'foreign'; },
  full_edge: f => { f.evidence.continuation_message_id = 'foreign'; },
  accepted_id: f => { const d = JSON.parse(f.accepted.data); d.opening.accepted_message_id = 'foreign'; f.accepted.data = JSON.stringify(d); },
  stream_owner: f => f.streamUrls.set(f.reconnect.requestId, 'http://127.0.0.1:1/api/conversations/foreign/stream'),
  durable_ACK: f => { f.row.messages = f.row.messages.filter(m => m.message_id !== ack.canonical_message_id); },
  repeated_client_id: f => { f.row.messages.push(f.row.messages.find(m => m.message_id === id)); },
  instruction_bytes: f => { f.instruction += ' '; },
  missing_outcome: f => { delete f.cached.outcome; },
  hash: f => { f.cached.raw_sha256 = 'wrong'; },
  byte_count: f => { f.cached.body_bytes++; },
  raw_data: f => { f.cached.data.canonical_root.transcript_row_id = 'foreign'; },
  oversized: f => { f.cached.raw += ' '.repeat(2 * 1024 * 1024); f.cached.body_bytes = Buffer.byteLength(f.cached.raw); f.cached.raw_sha256 = sha(f.cached.raw); },
  accepted_content: f => { const d = JSON.parse(f.accepted.data); d.message.content.text += 'wrong'; f.accepted.data = JSON.stringify(d); },
  reconnect_shape: f => { const d = JSON.parse(f.reconnect.data); d.messages.find(m => m.message_id === id).message_type = 'agent'; f.reconnect.data = JSON.stringify(d); },
  ACK_request_key: f => { f.chatRequest.message_id = 'foreign'; },
  ACK_request_content: f => { f.chatRequest.text += 'wrong'; },
  ACK_already_persisted: f => { f.ack.body.already_persisted = true; f.ack.raw = JSON.stringify(f.ack.body); },
  ACK_raw: f => { f.ack.raw = '{}'; },
  ACK_queued: f => { f.ack.body.queued = false; f.ack.raw = JSON.stringify(f.ack.body); },
  ACK_steering: f => { f.ack.body.steering = true; f.ack.raw = JSON.stringify(f.ack.body); },
  truncated: f => { f.cached.truncated = true; },
  negative_404: f => { f.cached.status = 404; },
})) test(`cache authority rejects ${name}`, () => { const f = facts(); validateGlobalCacheAuthority(a, f); mutate(f);
  assert.throws(() => validateGlobalCacheAuthority(a, f), error => {
    assert.equal(error.code, error.captureReason ? undefined : 'ERR_ASSERTION');
    if (['product', 'root', 'current_latest', 'full_roster', 'pending_key', 'full_edge'].includes(name)) assert.equal(error.captureReason, 'wrong_aggregate');
    else assert.notEqual(error.captureReason, 'wrong_aggregate', name);
    const guard = { root: /stable product\/root/, full_roster: /ordered full roster|latest_transcript|strictly equal/,
      product: /committed opening evidence/, current_latest: /ordered full roster/, pending_key: /committed opening evidence/, full_edge: /strictly equal/,
      accepted_id: /openingMatches/, stream_owner: /strictly equal/, durable_ACK: /durable composer ACK/, repeated_client_id: /unique durable opening/,
      instruction_bytes: /instruction bytes/, missing_outcome: /complete received cache body/, hash: /cache raw SHA256/, byte_count: /cache byte count/,
      raw_data: /cache raw\/data parity/, oversized: /cache body bound/, accepted_content: /instruction bytes/, reconnect_shape: /message shape/,
      ACK_raw: /deep-equal/, ACK_queued: /strictly equal/, ACK_steering: /steering/, ACK_already_persisted: /already_persisted/,
      ACK_request_key: /strictly equal/, ACK_request_content: /durable ACK request bytes/, truncated: /cache explicitly truncated/, negative_404: /strictly equal/ };
    assert.match(error.message, guard[name], name);
    return true;
  }); });
for (const [name, url, box] of [
  ['valid', selection, physical], ['wrong_pin', selection.replace(`source_transcript=${latest}`, 'source_transcript=foreign'), physical],
  ['wrong_hash', selection.replace(encodeURIComponent(id), 'foreign'), physical], ['wrong_route', selection.replace(`/global/${a.members[0]}`, '/global/foreign'), physical],
  ['duplicate_pin', selection.replace('#', `&source_transcript=${latest}#`), physical], ['duplicate_physical_ID', selection, { ...physical, count: 2 }],
  ['invisible', selection, { ...physical, visible: false }], ['wrong_physical_ID', selection, { ...physical, persistedMessageID: 'foreign' }],
]) test(`selection ${name} (UNIT ONLY physical observation)`, () => name === 'valid' ? validateGlobalChapterSelection(a, evidence, url, box) : assert.throws(() => validateGlobalChapterSelection(a, evidence, url, box)));
test('independent reference requires original projection at the real coordinator endpoint; retained Pending is not Accepted', () => {
  const url = 'http://127.0.0.1:1' + globalChapterReferencePath(a);
  assert.throws(() => validateGlobalChapterReference(a, evidence, cached.data, url));
  assert.throws(() => validateGlobalChapterReference(a, evidence, cached.data, url.replace('&boundary_representation=original_messages', '')));
  assert.throws(() => validateGlobalChapterReference(a, evidence, cached.data, url.replace(a.endpoint, '/api/product-conversations/' + a.product_conversation_id)));
});
test('pinned d4 routes and real source selection callback remain unchanged', () => {
  const root = process.env.HISTORY_PINNED_SOURCE ?? 'target/history-hidden-geometry-repair';
  const app = readFileSync(`${root}/ui/src/App.tsx`, 'utf8');
  assert.ok(app.includes('<Route path="/global/:slug" element={<CoordinatorPage />} />'));
  const coordinator = readFileSync(`${root}/ui/src/pages/CoordinatorPage.tsx`, 'utf8');
  assert.ok(coordinator.includes('api.resolveCoordinatorRoute(pin)')); assert.ok(coordinator.includes('api.getConversation(pin)'));
  assert.ok(coordinator.includes("const sourceSelectionKey = JSON.stringify([slug, sourcePins, sourcePins.length === 0 && sourceQuery.has('source_tool')]);"));
  assert.ok(coordinator.includes('if (resolvedCoordinatorId === null) {'));
  assert.ok(coordinator.includes('[fixtureData, navigate, slug, sourceSelectionKey, topologyRevision, location.search, resolvedCoordinatorId]'));
  const product = readFileSync(`${root}/ui/src/pages/ProductConversationPage.tsx`, 'utf8');
  assert.ok(product.includes('ownedSnapshot.productConversationId === productConversationId'));
  assert.ok(product.includes('snapshot.segments.find((candidate) => candidate.transcript_row_id === pin)'));
  const nav = readFileSync(`${root}/ui/src/components/ConversationNavStack.tsx`, 'utf8');
  const callback = nav.slice(nav.indexOf('  const selectChapterTarget ='), nav.indexOf('  const handleLoadOlderMessages ='));
  assert.ok(nav.includes('onJump={handleJump}'));
  assert.ok(readFileSync(`${root}/ui/src/components/ConversationNav.tsx`, 'utf8').includes(': onJump(chapter)'));
  let functionSource = callback.slice(callback.indexOf('(target: ChapterTarget) => {'), callback.indexOf('  const handleJump ='));
  functionSource = functionSource.slice(0, functionSource.indexOf('}, [location.pathname')) + '}';
  assert.ok(readFileSync(`${root}/ui/src/conversation/conversationChapters.ts`, 'utf8').includes("kind: 'persisted'"));
  const calls = [], location = { pathname: '/global/root', search: '?source_transcript=older&source_tool=old&filter=keep' };
  const select = Function('location', 'navigate', `return (${stripTypeScriptTypes(functionSource)});`)(location, (...args) => calls.push(args));
  select({ kind: 'persisted', transcriptId: latest, messageId: id });
  assert.deepEqual(calls[0][0], { pathname: location.pathname, search: `source_transcript=${latest}&filter=keep`, hash: `#message-${encodeURIComponent(id)}` });
  assert.ok(callback.includes('selectChapterTarget(target);'));
  assert.ok(callback.includes(': chapter.target;'));
  const driver = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  assert.ok(driver.includes("a.kind === 'ordinary' ? page.waitForResponse"));
  assert.ok(driver.indexOf('updateFault(fault, { failRoster: false });') < driver.indexOf('await chapter.click();'));
});

const shippingDir = process.env.HISTORY_SHIPPING_D4_EVIDENCE ?? '.phoenix/evidence/continuous-history/shipping-d4-artifact/history-evidence';
const shippingFixture = JSON.parse(readFileSync(`${shippingDir}/fixture.json`)).aggregates.find(a => a.kind === 'global');
const shippingRows = readFileSync(`${shippingDir}/driver-api.jsonl`, 'utf8').trim().split('\n').map(JSON.parse);
const receipt = shippingRows[21]; // authentic driver-api line22, not reconstructed from Pending
const expectedReceipt = {
  product_conversation_id: 'd4ba96e0-a169-4bea-af0f-4d4aade0acb5',
  predecessor_transcript_row_id: shippingFixture.members[1], successor_transcript_row_id: shippingFixture.members[2],
  continuation_message_id: shippingFixture.boundaries[1].message_id, client_turn_key: 'e400b377-5709-4239-8073-15426ef81642',
};
const shipping = { ...shippingFixture, product_conversation_id: expectedReceipt.product_conversation_id };
const referenceURL = 'http://127.0.0.1:1' + globalChapterReferencePath(shipping);
const referenceData = JSON.parse(receipt.response);
test('authentic shipping d4 Accepted original_messages independent reference positive', () => {
  assert.equal(receipt.status, 200); assert.equal(receipt.method, 'GET');
  assert.equal(receipt.path, shipping.endpoint + '?boundary_representation=original_messages');
  assert.notEqual(expectedReceipt.product_conversation_id, shipping.members[0]);
  assert.ok(shipping.occurrences.some(marker => marker[2] === expectedReceipt.successor_transcript_row_id + ':' + expectedReceipt.client_turn_key), 'independent fixture accepted member marker');
  validateGlobalChapterReference(shipping, expectedReceipt, referenceData, referenceURL);
});
for (const [name, mutate] of Object.entries({
  product: d => { d.product_conversation_id = 'foreign'; }, root: d => { d.canonical_root.transcript_row_id = 'foreign'; },
  latest: d => { d.latest_transcript_row_id = shipping.members[1]; },
  edge: d => { d.latest_opening.edge.continuation_message_id = 'foreign'; },
  roster: d => { d.segments.pop(); }, snapshot_projection: d => { delete d.boundaries; },
})) test(`independent Accepted reference rejects ${name} after genuine positive`, () => {
  validateGlobalChapterReference(shipping, expectedReceipt, referenceData, referenceURL);
  const data = structuredClone(referenceData); mutate(data);
  assert.throws(() => validateGlobalChapterReference(shipping, expectedReceipt, data, referenceURL), error => error.captureReason === 'wrong_aggregate');
});
