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
const seed = events.slice(0, events.indexOf(accepted)).findLast(e => e.requestId === accepted.requestId && e.eventName === 'init');
const reconnectAssertion = lines('assertions.jsonl').find(r => r.name === 'same-page transport SSE drop/reconnect preserves accepted full-edge identity');
const reconnect = reconnectAssertion.reconnect;
const cachedReceipt = lines('browser-responses.jsonl').find(r => r.raw && JSON.parse(r.raw).latest_opening?.client_turn_key === evidence.client_turn_key && JSON.parse(r.raw).segments.length === a.members.length + 1);
const cached = { ...cachedReceipt, data: JSON.parse(cachedReceipt.raw) };
const row = JSON.parse(lines('driver-http.jsonl').filter(r => r.method === 'GET' && r.path === `/api/conversations/${latest}`).at(-1).raw);
const ack = lines('assertions.jsonl').find(r => r.name === 'actual successor chat ACK receipt');
const instruction = lines('driver-http.jsonl').find(r => r.method === 'POST' && r.path.endsWith('/continue')).body.handoff;
const requests = lines('browser-requests.jsonl');
const chatRequest = JSON.parse(requests.find(r => r.method === 'POST' && new URL(r.url).pathname === `/api/conversations/${latest}/chat`).body);
const streamPath = `/api/conversations/${latest}/stream`;
const seedStream = requests.find(r => new URL(r.url).pathname === streamPath).url;
const reconnectStream = reconnectAssertion.requests.find(r => new URL(r.url).pathname === streamPath).url;
// Retained logs lack CDP requestWillBeSent mappings. These URL associations are an OFFLINE adapter,
// not new requestId provenance; live validation uses the actual CDP request map.
const sourceFacts = { evidence, cached, accepted, seed, reconnect, observedEvents: events,
  streamUrls: new Map([[accepted.requestId, seedStream], [reconnect.requestId, reconnectStream]]), row, ack, chatRequest, instruction };
const facts = () => structuredClone(sourceFacts);
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
test('tail reconnect uses the message-bearing authority branch (UNIT ONLY coverage variant)', () => {
  const candidate = facts();
  const payload = JSON.parse(candidate.reconnect.data);
  payload.transcript_coverage = 'tail';
  candidate.reconnect.data = JSON.stringify(payload);
  assert.equal(validateGlobalCacheAuthority(a, candidate).latest, latest);
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
  reconnect_duplicate: f => { const d = JSON.parse(f.reconnect.data); d.messages.push(d.messages.find(m => m.message_id === id)); f.reconnect.data = JSON.stringify(d); },
  reconnect_empty_complete: f => { const d = JSON.parse(f.reconnect.data); d.messages = []; f.reconnect.data = JSON.stringify(d); },
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
      accepted_id: /accepted event message identity|accepted publication full opening/, stream_owner: /owning CDP stream owner/, durable_ACK: /durable composer ACK/, repeated_client_id: /unique durable opening/,
      instruction_bytes: /instruction bytes/, missing_outcome: /complete received cache body/, hash: /cache raw SHA256/, byte_count: /cache byte count/,
      raw_data: /cache raw\/data parity/, oversized: /cache body bound/, accepted_content: /instruction bytes/, reconnect_shape: /message shape/,
      reconnect_duplicate: /unique reconnect opening/, reconnect_empty_complete: /complete\/tail reconnect must bear messages/,
      ACK_raw: /deep-equal/, ACK_queued: /strictly equal/, ACK_steering: /steering/, ACK_already_persisted: /already_persisted/,
      ACK_request_key: /strictly equal/, ACK_request_content: /durable ACK request bytes/, truncated: /cache explicitly truncated/, negative_404: /strictly equal/ };
    assert.match(error.message, guard[name], name);
    return true;
  }); });
const dir832 = process.env.HISTORY_GLOBAL_832_EVIDENCE
  ?? '.phoenix/evidence/continuous-history/instrumented832-artifact/opening-codegen-832d5ea50f6efba2bd3b926cac4d7229caa12a5f-1/opening-browser-evidence/global';
const lines832 = name => readFileSync(`${dir832}/${name}`, 'utf8').trim().split('\n').map(JSON.parse);
const a832 = JSON.parse(readFileSync(`${dir832}/fixture.json`));
const evidence832 = lines832('ipc.jsonl').find(record => record.reply.evidence).reply.evidence;
const latest832 = evidence832.successor_transcript_row_id;
const id832 = `${latest832}:${evidence832.client_turn_key}`;
const events832 = lines832('browser-sse.jsonl');
const accepted832 = events832.find(event => ['message', 'message_updated'].includes(event.eventName)
  && JSON.parse(event.data).opening?.accepted_message_id === id832);
const seed832 = events832.slice(0, events832.indexOf(accepted832))
  .findLast(event => event.requestId === accepted832.requestId && event.eventName === 'init');
const reconnectAssertion832 = lines832('assertions.jsonl')
  .find(record => record.name === 'same-page transport SSE drop/reconnect preserves accepted full-edge identity');
const reconnect832 = reconnectAssertion832.reconnect;
const cachedReceipt832 = lines832('browser-responses.jsonl').find(record => record.raw
  && JSON.parse(record.raw).latest_opening?.client_turn_key === evidence832.client_turn_key
  && JSON.parse(record.raw).segments.length === a832.members.length + 1);
const cached832 = { ...cachedReceipt832, data: JSON.parse(cachedReceipt832.raw) };
const row832 = JSON.parse(lines832('driver-http.jsonl')
  .filter(record => record.method === 'GET' && record.path === `/api/conversations/${latest832}`).at(-1).raw);
const ack832 = lines832('assertions.jsonl').find(record => record.name === 'actual successor chat ACK receipt');
const instruction832 = lines832('driver-http.jsonl').find(record => record.method === 'POST' && record.path.endsWith('/continue')).body.handoff;
const requests832 = lines832('browser-requests.jsonl');
const chatRequest832 = JSON.parse(requests832.find(record => record.method === 'POST'
  && new URL(record.url).pathname === `/api/conversations/${latest832}/chat`).body);
const streamPath832 = `/api/conversations/${latest832}/stream`;
const seedStream832 = requests832.find(record => new URL(record.url).pathname === streamPath832).url;
const reconnectRequest832 = reconnectAssertion832.requests.find(record => new URL(record.url).pathname === streamPath832);
const sourceFacts832 = { evidence: evidence832, cached: cached832, accepted: accepted832, seed: seed832,
  reconnect: reconnect832, observedEvents: events832,
  // OFFLINE retained-log adapter only: assertion request slice supplies the authentic reconnect URL,
  // while live qualification requires the CDP requestId map.
  streamUrls: new Map([[accepted832.requestId, seedStream832], [reconnect832.requestId, reconnectRequest832.url]]),
  row: row832, ack: ack832, chatRequest: chatRequest832, instruction: instruction832 };
const facts832 = () => structuredClone(sourceFacts832);
const changeEvent832 = (factsValue, key, change) => {
  const payload = JSON.parse(factsValue[key].data); change(payload); factsValue[key].data = JSON.stringify(payload);
};
const changeReconnectUrl832 = (factsValue, change) => {
  const url = new URL(factsValue.streamUrls.get(factsValue.reconnect.requestId));
  change(url); factsValue.streamUrls.set(factsValue.reconnect.requestId, url.href);
};

test('authentic 832 preserve reconnect chain passes authority offline; failed run is not qualification', () => {
  const candidate = facts832();
  assert.equal(candidate.seed.requestId, '14288.113');
  assert.equal(candidate.reconnect.requestId, '14288.127');
  assert.equal(JSON.parse(candidate.seed.data).transcript_coverage, 'complete');
  assert.equal(JSON.parse(candidate.seed.data).opening.status, 'pending');
  assert.deepEqual(JSON.parse(candidate.reconnect.data).messages, []);
  assert.equal(JSON.parse(candidate.reconnect.data).transcript_coverage, 'preserve');
  assert.equal(new URL(reconnectRequest832.url).searchParams.get('after_event_sequence'), '14');
  assert.equal(validateGlobalCacheAuthority(a832, candidate).latest, latest832);
  const result = JSON.parse(readFileSync(`${dir832}/browser-result.json`));
  assert.equal(result.status, 'failed');
  assert.match(result.error, /'preserve'[\s\S]*'complete'/);
});

const preserveNegatives = {
  missing_seed: [factsValue => { factsValue.seed = undefined; }, /accepted owning seed Init must precede/],
  fabricated_seed: [factsValue => { factsValue.seed = structuredClone(factsValue.seed); }, /accepted owning seed Init must precede|selected seed is the actual accepted owning prior Init/],
  wrong_seed_owner: [factsValue => changeEvent832(factsValue, 'seed', payload => { payload.conversation.id = 'foreign'; }), /accepted Init successor|seed Init owner/],
  wrong_seed_edge: [factsValue => changeEvent832(factsValue, 'seed', payload => { payload.opening.edge.continuation_message_id = 'foreign'; }), /accepted Init full edge|seed Init Pending full edge/],
  wrong_seed_coverage: [factsValue => changeEvent832(factsValue, 'seed', payload => { payload.transcript_coverage = 'preserve'; }), /seed Init complete or tail coverage/],
  crossgen_seed: [factsValue => changeEvent832(factsValue, 'seed', payload => { payload.transcript_generation = 2; }), /seed Init generation matches reconnect request/],
  crossgen_init: [factsValue => changeEvent832(factsValue, 'reconnect', payload => { payload.transcript_generation = 2; }), /reconnect Init generation matches reconnect request/],
  crossgen_row: [factsValue => { factsValue.row.conversation.transcript_generation = 2; }, /durable row generation matches reconnect request/],
  crossgen_request: [factsValue => changeReconnectUrl832(factsValue, url => url.searchParams.set('transcript_generation', '2')), /seed Init generation matches reconnect request/],
  incarnation: [factsValue => changeEvent832(factsValue, 'reconnect', payload => { payload.stream_incarnation = 'foreign'; }), /same stream incarnation/],
  missing_cursor: [factsValue => changeReconnectUrl832(factsValue, url => url.searchParams.delete('after_event_sequence')), /reconnect cursor exact single query value/],
  duplicate_cursor: [factsValue => changeReconnectUrl832(factsValue, url => url.searchParams.append('after_event_sequence', '14')), /reconnect cursor exact single query value/],
  malformed_cursor: [factsValue => changeReconnectUrl832(factsValue, url => url.searchParams.set('after_event_sequence', '014')), /reconnect cursor canonical nonnegative integer/],
  unsafe_cursor: [factsValue => changeReconnectUrl832(factsValue, url => url.searchParams.set('after_event_sequence', '9007199254740992')), /reconnect cursor safe integer/],
  missing_generation: [factsValue => changeReconnectUrl832(factsValue, url => url.searchParams.delete('transcript_generation')), /reconnect generation exact single query value/],
  duplicate_generation: [factsValue => changeReconnectUrl832(factsValue, url => url.searchParams.append('transcript_generation', '1')), /reconnect generation exact single query value/],
  malformed_generation: [factsValue => changeReconnectUrl832(factsValue, url => url.searchParams.set('transcript_generation', '1.0')), /reconnect generation canonical nonnegative integer/],
  unsafe_generation: [factsValue => changeReconnectUrl832(factsValue, url => url.searchParams.set('transcript_generation', '9007199254740992')), /reconnect generation safe integer/],
  regressed_cursor: [factsValue => changeReconnectUrl832(factsValue, url => url.searchParams.set('after_event_sequence', '0')), /reconnect cursor covers accepted publication/],
  unsafe_accepted_sequence: [factsValue => changeEvent832(factsValue, 'accepted', payload => { payload.sequence_id = 9007199254740992; }), /actual accepted wire sequence required/],
  anchor_after_cursor: [factsValue => changeEvent832(factsValue, 'reconnect', payload => { payload.pending_anchor_sequence_id = 15; }), /preserve reconnect anchor cannot exceed cursor/],
  cursor_after_tip: [factsValue => changeEvent832(factsValue, 'reconnect', payload => { payload.sequence_id = 13; payload.last_sequence_id = 13; }), /reconnect cursor cannot exceed tip/],
  truncated: [factsValue => changeEvent832(factsValue, 'reconnect', payload => { payload.pending_truncated = true; }), /reconnect pending events not truncated/],
  nonempty_preserve_messages: [factsValue => changeEvent832(factsValue, 'reconnect', payload => { payload.messages = [factsValue.row.messages[0]]; }), /preserve reconnect messages must be empty/],
  pending_order: [factsValue => changeEvent832(factsValue, 'reconnect', payload => {
    payload.sequence_id = 16; payload.last_sequence_id = 16;
    payload.pending_events = [{ type: 'agent_done', sequence_id: 16 }, { type: 'state_change', sequence_id: 15 }];
  }), /reconnect pending events ordered unique strictly after cursor/],
  pending_range: [factsValue => changeEvent832(factsValue, 'reconnect', payload => { payload.pending_events = [{ type: 'agent_done', sequence_id: 15 }]; }), /reconnect pending event cannot exceed tip/],
  missing_accepted_message: [factsValue => changeEvent832(factsValue, 'accepted', payload => { payload.message_id = payload.message.message_id; delete payload.message; }), /full accepted publication message required/],
  foreign_accepted_owner: [factsValue => changeEvent832(factsValue, 'accepted', payload => { payload.message.conversation_id = 'foreign'; }), /accepted\/reconnect message owner/],
  foreign_accepted_opening: [factsValue => changeEvent832(factsValue, 'accepted', payload => { payload.opening.edge.continuation_message_id = 'foreign'; }), /accepted Init full edge|accepted publication full opening/],
  foreign_reconnect_opening: [factsValue => changeEvent832(factsValue, 'reconnect', payload => { payload.opening.edge.continuation_message_id = 'foreign'; }), /reconnect Init full opening/],
  accepted_content: [factsValue => changeEvent832(factsValue, 'accepted', payload => { payload.message.content.text += 'foreign'; }), /accepted\/reconnect instruction bytes/],
  durable_content: [factsValue => { factsValue.row.messages.find(message => message.message_id === id832).content.text += 'foreign'; }, /durable instruction bytes/],
};
for (const [name, [mutate, guard]] of Object.entries(preserveNegatives)) {
  test(`832 preserve authority rejects ${name} at responsible guard`, () => {
    const candidate = facts832();
    validateGlobalCacheAuthority(a832, candidate);
    mutate(candidate);
    assert.throws(() => validateGlobalCacheAuthority(a832, candidate), guard);
  });
}

for (const [name, url, box] of [
  ['valid', selection, physical], ['wrong_pin', selection.replace(`source_transcript=${latest}`, 'source_transcript=foreign'), physical],
  ['wrong_hash', selection.replace(encodeURIComponent(id), 'foreign'), physical], ['wrong_route', selection.replace(`/global/${a.members[0]}`, '/global/foreign'), physical],
  ['duplicate_pin', selection.replace('#', `&source_transcript=${latest}#`), physical], ['duplicate_physical_ID', selection, { ...physical, count: 2 }],
  ['invisible', selection, { ...physical, visible: false }], ['wrong_physical_ID', selection, { ...physical, persistedMessageID: 'foreign' }],
]) test(`selection ${name} (UNIT ONLY physical observation)`, () => name === 'valid' ? validateGlobalChapterSelection(a, evidence, url, box, new URL(selection).pathname) : assert.throws(() => validateGlobalChapterSelection(a, evidence, url, box, new URL(selection).pathname)));
test('actual chapter must preserve pre-click alias, not switch to another known member/product/latest alias', () => {
  const before = new URL(selection).pathname;
  validateGlobalChapterSelection(a, evidence, selection, physical, before);
  for (const alias of [a.product_conversation_id, ...a.members.slice(1), latest]) {
    const changed = new URL(selection); changed.pathname = `/global/${alias}`;
    if (changed.pathname === before) continue;
    assert.throws(() => validateGlobalChapterSelection(a, evidence, changed.href, physical, before), /exact pre-click route alias/);
  }
  assert.throws(() => validateGlobalChapterSelection(a, evidence, selection, physical), /actual pre-click pathname required/);
  const driver = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  assert.ok(driver.indexOf('const chapterPreClickUrl = page.url()') < driver.indexOf('await chapter.click()'));
  assert.match(driver, /new URL\(chapterPreClickUrl\)\.pathname/);
});

test('selection receipt revalidates the actual final URL after async reference/drain and records measured URLs', () => {
  const driver = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  assert.match(driver, /const chapterValidatedUrl = page\.url\(\)/);
  assert.match(driver, /const chapterReceiptUrl = chapterFinalPhysical\.url/);
  assert.match(driver, /url: window\.location\.href, browser_at_millis: Date\.now\(\), count: nodes\.length/);
  assert.match(driver, /validateGlobalChapterSelection\(a, entered, chapterReceiptUrl/);
  assert.match(driver, /assert\.equal\(chapterReceiptUrl, chapterValidatedUrl, 'selected chapter URL remains stable/);
  assert.match(driver, /validated_url: chapterValidatedUrl, url: chapterReceiptUrl/);
  const finalRead = driver.indexOf('const chapterFinalPhysical = await page.evaluate(');
  const receipt = driver.indexOf("checked('Global cache-backed chapter selection");
  assert.ok(driver.lastIndexOf('await recorder.sealAndDrain()', finalRead) < finalRead && finalRead < receipt);
  assert.doesNotMatch(driver.slice(receipt, receipt + 300), /url: page\.url\(\)/);
  const changed = new URL(selection); changed.pathname = `/global/${a.members[1]}`;
  assert.throws(() => validateGlobalChapterSelection(a, evidence, changed.href, physical, new URL(selection).pathname), /exact pre-click route alias/);
});

test('final receipt uses the actual atomic URL/identity/viewport producer after asynchronous route changes (UNIT ONLY DOM)', () => {
  const driver = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const start = driver.indexOf('const chapterFinalPhysical = await page.evaluate(') + 'const chapterFinalPhysical = await page.evaluate('.length;
  const end = driver.indexOf(', { selector: persistedMessageSelector(acceptedId) });', start);
  assert.ok(start > 0 && end > start);
  const produce = new Function('document', 'window', `return (${driver.slice(start, end)});`);
  const node = { id: `message-${id}`, getAttribute: () => null, checkVisibility: options => options.visibilityProperty === true,
    getBoundingClientRect: () => ({ top: 10, bottom: 30, width: 100, height: 20 }) };
  const document = { querySelectorAll: () => [node], querySelector: () => ({ getBoundingClientRect: () => ({ top: 0, bottom: 100 }) }) };
  const window = { location: { href: selection } };
  const callback = produce(document, window);
  const measured = callback({ selector: 'exact-ID' });
  validateGlobalChapterSelection(a, evidence, measured.url, measured, new URL(selection).pathname);
  const changed = new URL(selection); changed.pathname = `/global/${a.members[1]}`; window.location.href = changed.href;
  const later = callback({ selector: 'exact-ID' });
  assert.equal(later.url, changed.href);
  assert.throws(() => validateGlobalChapterSelection(a, evidence, later.url, later, new URL(selection).pathname), /exact pre-click route alias/);
  document.querySelectorAll = () => [node, { ...node }];
  const duplicate = callback({ selector: 'exact-ID' });
  assert.throws(() => validateGlobalChapterSelection(a, evidence, duplicate.url, duplicate, new URL(changed.href).pathname));
  window.location.href = selection;
  document.querySelectorAll = () => [{ ...node, checkVisibility: () => false }];
  const hidden = callback({ selector: 'exact-ID' });
  assert.equal(hidden.visible, false);
  assert.throws(() => validateGlobalChapterSelection(a, evidence, hidden.url, hidden, new URL(selection).pathname));
});

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
