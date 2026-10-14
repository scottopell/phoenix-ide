/** INSTRUMENTED actual-router witness. No app imports, public controls or fabricated HTTP/SSE. */
import assert from 'node:assert/strict';
import { appendFileSync, readFileSync, writeFileSync, renameSync, statSync, lstatSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve, dirname } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createConnection } from 'node:net';
import { createHash, randomUUID } from 'node:crypto';
import { loopback, sourceLink, anchorMatches, COMPOSER_SELECTOR, belongsToAggregate, aggregateBoundaries } from './qualify-history-realserver.mjs';

export function ownedWireCursor(event, observedEvents) {
  const payload = JSON.parse(event.data);
  assert.ok(Number.isSafeInteger(payload.sequence_id) && payload.sequence_id >= 0, 'actual accepted wire sequence required');
  const index = observedEvents.indexOf(event);
  assert.ok(index >= 0 && typeof event.requestId === 'string', 'accepted owning CDP request required');
  const init = observedEvents.slice(0, index).findLast(candidate => candidate.requestId === event.requestId && candidate.eventName === 'init');
  assert.ok(init, 'accepted event must belong to an observed Init request');
  const initial = JSON.parse(init.data);
  assert.match(initial.stream_incarnation, /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
  assert.ok(Number.isSafeInteger(initial.sequence_id) && payload.sequence_id > initial.sequence_id, 'accepted wire cursor must advance its owning Init');
  return { request_id: event.requestId, stream_incarnation: initial.stream_incarnation,
    init_sequence_id: initial.sequence_id, sequence_id: payload.sequence_id,
    transport_event_id: event.eventId };
}

export function acceptedWireCursor(event, observedEvents) {
  const cursor = ownedWireCursor(event, observedEvents);
  const payload = JSON.parse(event.data);
  const index = observedEvents.indexOf(event);
  const initial = JSON.parse(observedEvents.slice(0, index).findLast(e => e.requestId === event.requestId && e.eventName === 'init').data);
  assert.equal(payload.opening?.status, 'accepted');
  assert.ok(sameEdge(initial.opening?.edge, payload.opening?.edge), 'accepted Init full edge');
  assert.equal(initial.conversation?.id, payload.opening?.edge.successor_transcript_row_id, 'accepted Init successor');
  assert.equal(payload.message?.message_id ?? payload.message_id, payload.opening?.accepted_message_id, 'accepted event message identity');
  return cursor;
}

export function persistedMessageSelector(id) {
  assert.ok(typeof id === 'string' && id.length > 0, 'exact persisted message identity required');
  return `#messages [id=${JSON.stringify(`message-${id}`)}], #messages [data-message-id=${JSON.stringify(id)}]`;
}

const EDGE_FIELDS = ['product_conversation_id', 'predecessor_transcript_row_id', 'successor_transcript_row_id', 'continuation_message_id'];
export function sameEdge(a, b) {
  return !!a && !!b && EDGE_FIELDS.every(key => typeof a[key] === 'string' && a[key].length > 0 && a[key] === b[key]);
}
export function openingMatches(value, evidence, status) {
  return value?.status === status && sameEdge(value.edge, evidence)
    && value.opening_authority === 'user_authorized_instruction'
    && (status === 'pending' ? value.client_turn_key === evidence.client_turn_key
      : value.accepted_message_id === `${evidence.successor_transcript_row_id}:${evidence.client_turn_key}`
        && value.presentation === 'separate_message');
}
export function validateOwnedAggregate(a, data, url, structuralReservation = null) {
  const fail = message => { throw Object.assign(new Error(message), { captureReason: 'wrong_aggregate' }); };
  try {
    assert.ok(belongsToAggregate(a, { url, data }), 'stable product/root/request ownership');
    if (structuralReservation) {
      const r = structuralReservation, o = data.latest_opening;
      assert.equal(r.kind, 'expected_manual_pending');
      assert.equal(o?.status, 'pending', 'only Pending may stage');
      assert.equal(o.opening_authority, r.opening_authority);
      assert.equal(o.edge.product_conversation_id, r.product_conversation_id);
      assert.equal(o.edge.predecessor_transcript_row_id, r.predecessor_transcript_row_id);
      assert.equal(o.client_turn_key, r.client_turn_key);
      assert.equal(data.segments?.length, a.members.length + 1, 'one reserved future member only');
      assert.ok(!a.members.includes(o.edge.successor_transcript_row_id), 'future successor required');
      assert.ok(EDGE_FIELDS.every(k => typeof o.edge[k] === 'string' && o.edge[k].length > 0));
      a = { ...a, members: [...a.members, o.edge.successor_transcript_row_id],
        opening_evidence: { ...a.opening_evidence, [o.edge.successor_transcript_row_id]: { ...o.edge, client_turn_key: r.client_turn_key } } };
    }
    const segments = data.segments;
    assert.ok(Array.isArray(segments) && segments.length > 0 && segments.length <= a.members.length);
    assert.deepEqual(segments.map(s => s.transcript_row_id), a.members.slice(0, segments.length), 'ordered full roster');
    segments.forEach((s, i) => {
      assert.equal(s.segment_ordinal, i); assert.ok(Array.isArray(s.messages));
      s.messages.forEach(m => { assert.ok(typeof m.message_id === 'string' && m.message_id.length > 0); assert.ok(Number.isSafeInteger(m.sequence_id) && m.sequence_id >= 0); });
    });
    assert.equal(data.latest_transcript_row_id, segments.at(-1).transcript_row_id);
    assert.ok(segments.some(s => s.transcript_row_id === data.requested_transcript_row_id));
    const path = new URL(url).pathname;
    if (a.kind === 'ordinary' && path === a.endpoint) assert.equal(data.requested_transcript_row_id, a.members[0], 'stable product route selects root');
    if (a.kind === 'global' || path !== a.endpoint) assert.equal(decodeURIComponent(path.split('/').at(-1)), data.requested_transcript_row_id, 'selected alias');
    const original = new URL(url).searchParams.get('boundary_representation') === 'original_messages';
    if (a.kind === 'global' || original) {
      assert.ok(Array.isArray(data.boundaries), 'original/Global boundary wire');
      assert.ok(segments.every(s => !Object.hasOwn(s, 'handoff')), 'original history segment wire');
    } else {
      assert.equal(Object.hasOwn(data, 'boundaries'), false, 'ordinary snapshot uses segments[].handoff');
      assert.ok(segments.every(s => Object.hasOwn(s, 'handoff')), 'snapshot handoff wire');
    }
    const boundaries = aggregateBoundaries(data);
    boundaries.forEach(b => {
      assert.ok(Number.isInteger(b.segment_ordinal) && b.segment_ordinal >= 0);
      assert.equal(b.predecessor_transcript_row_id, segments[b.segment_ordinal]?.transcript_row_id);
      assert.equal(b.successor_transcript_row_id, segments[b.segment_ordinal + 1]?.transcript_row_id);
      assert.ok(typeof b.message_id === 'string' && b.message_id.length > 0);
      assert.ok(b.represented_successor_opening_message_id === null || b.represented_successor_opening_message_id?.startsWith(b.successor_transcript_row_id + ':'));
    });
    boundaries.forEach(b => {
      const known = a.boundaries?.find(candidate => candidate.segment_ordinal === b.segment_ordinal);
      const evidence = a.opening_evidence?.[b.successor_transcript_row_id];
      if (known) { assert.equal(b.message_id, known.message_id); assert.equal(b.represented_successor_opening_message_id, known.represented_successor_opening_message_id); }
      if (evidence) assert.equal(b.message_id, evidence.continuation_message_id);
      const h = segments[b.segment_ordinal].handoff;
      if (h?.kind === 'completed') {
        const acceptedId = evidence ? `${evidence.successor_transcript_row_id}:${evidence.client_turn_key}`
          : a.manual_opening.edge.successor_transcript_row_id === b.successor_transcript_row_id ? a.manual_opening.accepted_message_id : null;
        if (acceptedId) assert.equal(h.accepted_successor_message_id, acceptedId);
      }
    });
    const opening = data.latest_opening;
    assert.ok(['pending', 'accepted'].includes(opening?.status), 'manual opening union');
    assert.equal(opening.edge.product_conversation_id, a.product_conversation_id ?? a.manual_opening.edge.product_conversation_id);
    assert.equal(opening.edge.successor_transcript_row_id, data.latest_transcript_row_id);
    assert.equal(opening.edge.predecessor_transcript_row_id, segments.at(-2)?.transcript_row_id);
    assert.equal(opening.opening_authority, 'user_authorized_instruction');
    const expected = a.opening_evidence?.[data.latest_transcript_row_id];
    if (expected) assert.ok(openingMatches(opening, expected, opening.status), 'committed opening evidence');
    else { assert.equal(opening.status, 'accepted'); assert.deepEqual(opening, a.manual_opening, 'fixture manual opening identity'); }
    if (opening.status === 'pending') assert.ok(typeof opening.client_turn_key === 'string' && opening.client_turn_key.length > 0);
    else { assert.equal(opening.presentation, 'separate_message'); assert.ok(opening.accepted_message_id?.startsWith(data.latest_transcript_row_id + ':')); }
    const edgeBoundary = boundaries.find(b => b.message_id === opening.edge.continuation_message_id);
    if (a.kind === 'global' || original) assert.ok(edgeBoundary, 'original full boundary metadata');
    if (edgeBoundary) {
      assert.equal(edgeBoundary.predecessor_transcript_row_id, opening.edge.predecessor_transcript_row_id);
      assert.equal(edgeBoundary.successor_transcript_row_id, opening.edge.successor_transcript_row_id);
      assert.equal(edgeBoundary.represented_successor_opening_message_id, null);
    }
    return { product_conversation_id: data.product_conversation_id, canonical_root_transcript_row_id: data.canonical_root.transcript_row_id,
      requested_transcript_row_id: data.requested_transcript_row_id, latest_transcript_row_id: data.latest_transcript_row_id };
  } catch (error) { fail(error.message); }
}
export function validateChatAck(receipt, successor, key) {
  assert.equal(new URL(receipt.url).pathname, `/api/conversations/${successor}/chat`);
  assert.equal(receipt.status, 200);
  assert.equal(receipt.canonical_message_id, `${successor}:${key}`);
  assert.equal(receipt.body.queued, true);
  // Pinned ChatResponse omits false steering/already_persisted fields.
  for (const field of ['steering', 'already_persisted']) {
    if (Object.hasOwn(receipt.body, field)) assert.equal(receipt.body[field], false, field);
  }
  assert.deepEqual(JSON.parse(receipt.raw), receipt.body);
}
export function faultTarget(method, url, state, kind) {
  const parsed = new URL(url, 'http://127.0.0.1:1');
  const path = parsed.pathname;
  if (method !== 'GET' || parsed.protocol !== 'http:' || parsed.hostname !== '127.0.0.1' || parsed.username || parsed.password) return false;
  if (kind === 'stream') return /^\/api\/conversations\/[^/]+\/stream$/.test(path) && path === state.streamPath;
  // The pinned app has no /history-roster route: its roster refresh is this bounded aggregate GET.
  return state.failRoster === true && path === state.aggregatePath
    && path !== '/api/product-conversations/events'
    && (/^\/api\/product-conversations\/[^/]+$/.test(path)
      || /^\/api\/global\/coordinator\/history\/[^/]+$/.test(path));
}
export function ownedFile(path) {
  const parent = statSync(dirname(path));
  assert.ok(parent.isDirectory() && (parent.mode & 0o077) === 0 && parent.uid === process.getuid(), 'control root must be owner-only');
  assert.ok(lstatSync(path).isFile() && !lstatSync(path).isSymbolicLink(), 'existing non-symlink owned control file required');
  return path;
}
export function updateFault(path, values) {
  ownedFile(path);
  const next = { ...JSON.parse(readFileSync(path, 'utf8')), ...values };
  const staging = path + '.next';
  writeFileSync(staging, JSON.stringify(next), { mode: 0o600 });
  renameSync(staging, path);
}
export function previewTransport(base, file, log) {
  loopback(base); ownedFile(file);
  const read = () => JSON.parse(readFileSync(file, 'utf8'));
  return { '/api': { target: base, xfwd: true,
    bypass(req, res) {
      if (faultTarget(req.method, req.url, read(), 'roster')) {
        appendFileSync(log, JSON.stringify({ action: 'destroy-roster', method: req.method, url: req.url }) + '\n');
        res.destroy(); req.destroy();
        return false;
      }
    },
    configure(proxy) {
      const streams = new Set();
      let previous = read().drop;
      const timer = setInterval(() => {
        const state = read();
        if (state.drop === previous) return;
        previous = state.drop;
        let count = 0;
        for (const entry of streams) {
          if (!faultTarget(entry.method, entry.url, state, 'stream')) continue;
          count++; entry.upstream.destroy(); entry.downstream.destroy(); streams.delete(entry);
        }
        appendFileSync(log, JSON.stringify({ action: 'drop-streams', token: previous, count, path: state.streamPath }) + '\n');
      }, 25);
      timer.unref();
      proxy.on('proxyRes', (upstream, req, downstream) => {
        if (!faultTarget(req.method, req.url, read(), 'stream')) return;
        const entry = { method: req.method, url: req.url, upstream, downstream };
        streams.add(entry);
        downstream.once('close', () => streams.delete(entry));
      });
      proxy.once('close', () => { clearInterval(timer); for (const s of streams) { s.upstream.destroy(); s.downstream.destroy(); } });
    },
  }, '/preview': { target: base } };
}
export function ipc(path, command) {
  return new Promise((resolveReply, reject) => {
    const bytes = JSON.stringify(command) + '\n';
    assert.ok(Buffer.byteLength(bytes) <= 1024);
    const socket = createConnection(path);
    let raw = '', done = false;
    const finish = (error, value) => {
      if (done) return; done = true; socket.destroy();
      if (error) reject(error); else resolveReply(value);
    };
    socket.setTimeout(2000, () => finish(new Error('owned IPC deadline')));
    socket.once('connect', () => socket.write(bytes));
    socket.on('error', error => finish(error));
    socket.on('end', () => finish(new Error('IPC closed without receipt')));
    socket.on('data', chunk => {
      raw += chunk;
      if (Buffer.byteLength(raw) > 16384) return finish(new Error('IPC receipt bound'));
      if (!raw.includes('\n')) return;
      try { const reply = JSON.parse(raw.split('\n')[0]); assert.equal(reply.ok, true, reply.error); finish(null, reply.result); }
      catch (error) { finish(error); }
    });
  });
}
export async function releaseOnce(state, send) {
  if (!state.armed || state.releaseAttempted) return;
  state.releaseAttempted = true;
  assert.equal((await send({ command: 'release', predecessor: state.predecessor, client_turn_key: state.key })).state, 'released');
}

/** @typedef {{kind: 'required_aggregate_json'} | {kind: 'metadata_only'}} CapturePolicy */
export const REQUIRED_BODY_DEADLINE_MS = 11000;
export const REQUIRED_BODY_MAX_BYTES = 2 * 1024 * 1024;

/** @returns {CapturePolicy} */
export function capturePolicy(aggregate, method, url) {
  const path = new URL(url).pathname;
  const prefix = aggregate.kind === 'global' ? '/api/global/coordinator/history/' : '/api/product-conversations/';
  const owned = [aggregate.endpoint, ...aggregate.members.map(id => prefix + encodeURIComponent(id))];
  return method === 'GET' && owned.includes(path)
    ? { kind: 'required_aggregate_json' } : { kind: 'metadata_only' };
}

// Playwright request ordinals are local identities, not CDP Network.requestId.
export function installAggregateRecorder(page, aggregate, record, snapshots, {
  deadlineMs = REQUIRED_BODY_DEADLINE_MS, maxBytes = REQUIRED_BODY_MAX_BYTES,
  expectedRequestFailure = () => false, requestStamp = () => null,
  clock = { now: () => performance.now(), setTimeout, clearTimeout },
} = {}) {
  aggregate = { ...aggregate, members: [...aggregate.members], opening_evidence: { ...aggregate.opening_evidence } };
  let reservation = null, reservationEpoch = 0;
  const requests = [], failed = [], applicationErrors = [], captureErrors = [];
  const identities = new WeakMap(), required = new Map(), tasks = new Set();
  let ordinal = 0, navigationEpoch = 0, sealed = false, drain, drainDeadline, drainTimer, drainComplete = false;
  let wake;
  const notify = () => { wake?.(); wake = undefined; };
  const pending = () => [...required.values()].filter(s => ['awaiting_response', 'reading_body', 'awaiting_independent_committed_evidence'].includes(s.outcome));
  const inventory = (partial) => ({ sealed, partial, pending_required: pending().length, pending_body_tasks: tasks.size,
    pending_metadata: pending().map(s => ({ ...s.metadata, outcome: s.outcome })),
    drain_deadline: drainDeadline ?? null,
    required_outcomes: [...required.values()].map(({ metadata, outcome, expected_transport_failure }) => ({ ...metadata, outcome, expected_transport_failure })),
    application_page_errors: applicationErrors, cdp_capture_errors: captureErrors });
  const outside = (req, event) => record('browser-outside-capture.jsonl', { policy: { kind: 'outside_capture' },
    event, reason: 'unknown_post_seal_request', url: req.url(), method: req.method() });
  const identity = req => {
    if (!identities.has(req)) identities.set(req, Object.freeze({
      request_ordinal: ++ordinal, identity_source: 'playwright_request_local_ordinal',
      navigation_epoch: navigationEpoch, url: req.url(), method: req.method(), at: clock.now(),
      raw_request_stamp: structuredClone(requestStamp(req)),
      status: null, type: null, response_at: null,
    }));
    return identities.get(req);
  };
  const emitFailure = (metadata, reason, error, extra = {}) => {
    const entry = { ...metadata, ...extra, outcome: 'capture_failed', reason, error: String(error) };
    captureErrors.push(entry); record('browser-capture-errors.jsonl', entry);
    record('browser-responses.jsonl', entry);
  };
  const onRequest = req => {
    if (sealed) { outside(req, 'request'); return; }
    const metadata = identity(req), policy = capturePolicy(aggregate, metadata.method, metadata.url);
    const entry = { ...metadata, body: req.postData(), policy };
    requests.push(entry); record('browser-requests.jsonl', entry);
    if (policy.kind === 'required_aggregate_json') required.set(metadata.request_ordinal, { metadata, reservation, outcome: 'awaiting_response', expected_transport_failure: expectedRequestFailure(entry) });
  };
  const onFailed = req => {
    if (sealed && !identities.has(req)) { outside(req, 'requestfailed'); return; }
    const entry = { ...identity(req), failure: req.failure(), outcome: 'request_failed' };
    failed.push(entry); record('browser-failed.jsonl', entry);
    const state = required.get(entry.request_ordinal);
    if (state?.outcome === 'awaiting_response') {
      state.outcome = 'request_failed';
      record('browser-required-transport.jsonl', { ...entry, expected_transport_failure: state.expected_transport_failure });
      notify();
    }
  };
  const onPageError = error => {
    const entry = { error: String(error), at: performance.now(), navigation_epoch: navigationEpoch };
    applicationErrors.push(entry); record('page-errors.jsonl', entry);
  };
  const onNavigation = frame => { if (frame === page.mainFrame()) navigationEpoch++; };
  const onResponse = response => {
    if (!new URL(response.url()).pathname.startsWith('/api/')) return;
    const req = response.request();
    if (sealed && !identities.has(req)) { outside(req, 'response'); return; }
    const requestIdentity = identity(req);
    if (sealed && clock.now() >= drainDeadline) { terminatePending('deadline'); return; }
    const prior = required.get(requestIdentity.request_ordinal);
    if (prior && prior.outcome !== 'awaiting_response') return;
    const metadata = Object.freeze({ ...requestIdentity, url: response.url(), status: response.status(),
      type: response.headers()['content-type'] ?? '', response_at: clock.now(), reservation_epoch: prior?.reservation?.epoch ?? null,
      response_navigation_epoch: navigationEpoch });
    const policy = prior ? { kind: 'required_aggregate_json' } : { kind: 'metadata_only' };
    record('browser-responses.jsonl', { ...metadata, policy, outcome: 'response_metadata' });
    if (policy.kind === 'metadata_only') {
      record('browser-responses.jsonl', { ...metadata, policy, outcome: 'metadata_only', consumer: 'api_metadata_no_response_body' });
      return;
    }
    const state = { ...prior, metadata, outcome: 'reading_body' };
    required.set(metadata.request_ordinal, state);
    let timer, abort, timeout;
    const disposed = new Promise((_, reject) => { abort = reject; });
    state.dispose = (reason = 'disposed') => abort(Object.assign(new Error(`required receipt ${reason}`), { captureReason: reason }));
    const bodyDeadline = clock.now() + deadlineMs;
    const task = (async () => {
      let raw;
      try {
        assert.equal(requestIdentity.url, metadata.url, 'original request URL ownership');
        assert.equal(metadata.status, 200, 'required aggregate HTTP status');
        assert.match(metadata.type, /application\/json/i, 'required aggregate JSON content type');
        const remaining = Math.max(0, Math.min(bodyDeadline, drainDeadline ?? Infinity) - clock.now());
        timeout = new Promise((_, reject) => { timer = clock.setTimeout(() => reject(Object.assign(
          new Error(`required body deadline ${remaining}ms`), { captureReason: 'deadline' })), remaining); });
        try { raw = await Promise.race([Promise.resolve().then(() => response.text()), timeout, disposed]); }
        catch (error) { error.captureReason ??= 'body_transport'; throw error; }
        if (clock.now() >= Math.min(bodyDeadline, drainDeadline ?? Infinity)) {
          throw Object.assign(new Error('required receipt absolute deadline'), { captureReason: 'deadline' });
        }
        const bytes = Buffer.byteLength(raw);
        if (bytes > maxBytes) {
          state.outcome = 'capture_failed';
          emitFailure(metadata, 'over_limit', `required body ${bytes} exceeds ${maxBytes} bytes`, { body_bytes: bytes, max_bytes: maxBytes });
          return;
        }
        const raw_sha256 = createHash('sha256').update(raw).digest('hex');
        state.raw_sha256 = raw_sha256;
        record('browser-responses.jsonl', { ...metadata, outcome: 'body_received', raw, raw_sha256, body_bytes: bytes });
        let data;
        try { data = JSON.parse(raw); }
        catch (error) { error.captureReason = 'parse'; throw error; }
        let parsed_identity;
        const future = data.segments?.some(s => !aggregate.members.includes(s.transcript_row_id));
        if (future) {
          assert.ok(prior?.reservation, 'unreserved future aggregate');
          assert.ok(pending().filter(s => s.candidate).length < 300, 'staged receipt bound');
          parsed_identity = validateOwnedAggregate(aggregate, data, metadata.url, prior.reservation);
          state.candidate = data.latest_opening;
          state.outcome = 'awaiting_independent_committed_evidence';
          record('browser-responses.jsonl', { ...metadata, raw_sha256, body_bytes: bytes,
            outcome: state.outcome, validation: 'structural_candidate', absolute_deadline: bodyDeadline });
          const committed = new Promise(resolve => { state.commit = resolve; });
          await Promise.race([committed, timeout, disposed]);
          assert.ok(clock.now() < Math.min(bodyDeadline, drainDeadline ?? Infinity), 'required receipt absolute deadline');
        }
        parsed_identity = validateOwnedAggregate(aggregate, data, metadata.url);
        assert.ok(snapshots.length < 300, 'aggregate snapshot bound');
        snapshots.push({ ...metadata, raw, data, parsed_identity, raw_sha256, committed_evidence_source: state.committed_evidence_source ?? null });
        state.outcome = 'captured';
        record('browser-responses.jsonl', { ...metadata, outcome: 'captured', parsed_identity, raw_sha256, body_bytes: bytes, committed_evidence_source: state.committed_evidence_source ?? null });
      } catch (error) {
        state.outcome = 'capture_failed';
        emitFailure(metadata, error.captureReason ?? 'required_contract', error, { raw_sha256: state.raw_sha256 ?? null });
      } finally { clock.clearTimeout(timer); delete state.dispose; delete state.commit; }
    })();
    tasks.add(task);
    task.then(() => { tasks.delete(task); notify(); }, error => {
      tasks.delete(task); state.outcome = 'capture_failed';
      captureErrors.push({ ...metadata, reason: 'recorder_write', error: String(error) }); notify();
    });
  };
  const terminatePending = reason => {
    for (const state of pending()) {
      if (state.outcome === 'awaiting_response') {
        state.outcome = 'capture_failed'; emitFailure(state.metadata, reason, `required request ${reason} before response`);
      } else state.dispose?.(reason);
    }
    notify();
  };
  const onClose = () => terminatePending('disposed');
  page.on('request', onRequest); page.on('requestfailed', onFailed); page.on('response', onResponse);
  page.on('pageerror', onPageError); page.on('framenavigated', onNavigation); page.on('close', onClose);
  return { requests, failed, applicationErrors, captureErrors,
    partialInventory() { return inventory(true); },
    /** @param {{kind:'expected_manual_pending', product_conversation_id:string, predecessor_transcript_row_id:string, client_turn_key:string, opening_authority:'user_authorized_instruction'}} expected */
    reserveOpening(expected) {
      assert.ok(!sealed && !reservation, 'one prearmed reservation only');
      assert.equal(expected.kind, 'expected_manual_pending');
      assert.equal(expected.product_conversation_id, aggregate.manual_opening.edge.product_conversation_id);
      assert.equal(expected.predecessor_transcript_row_id, aggregate.members.at(-1));
      assert.equal(expected.opening_authority, 'user_authorized_instruction');
      assert.ok(typeof expected.client_turn_key === 'string' && expected.client_turn_key.length > 0);
      reservation = Object.freeze({ ...expected, epoch: ++reservationEpoch });
      record('browser-authority.jsonl', { outcome: 'prearmed_reservation', reservation });
    },
    registerCommittedOpening(evidence, source) {
      try {
        assert.ok(reservation, 'independent prearmed reservation required');
        assert.equal(evidence.edge, 'committed_intent_before_first_message_persistence', 'full committed IPC Pending evidence required');
        assert.ok(source?.kind === 'owned_ipc' && source.command === 'entered', 'independent IPC source receipt required');
        assert.equal(source.evidence?.edge, 'committed_intent_before_first_message_persistence', 'source Pending evidence required');
        assert.ok(sameEdge(source.evidence, evidence) && source.evidence.client_turn_key === evidence.client_turn_key, 'IPC source receipt identity');
        assert.ok(EDGE_FIELDS.every(k => typeof evidence[k] === 'string' && evidence[k].length > 0));
        for (const k of ['product_conversation_id', 'predecessor_transcript_row_id', 'client_turn_key']) assert.equal(evidence[k], reservation[k]);
        assert.equal(evidence.predecessor_transcript_row_id, aggregate.members.at(-1));
        assert.ok(!aggregate.members.includes(evidence.successor_transcript_row_id));
        for (const state of required.values()) if (state.candidate) {
          assert.notEqual(state.outcome, 'capture_failed', 'failed staged receipt cannot be revived');
          assert.ok(clock.now() < Math.min(state.metadata.response_at + deadlineMs, drainDeadline ?? Infinity), 'IPC evidence arrived late');
          assert.ok(openingMatches(state.candidate, evidence, 'pending'), 'candidate exact full committed edge/key');
        }
        aggregate = { ...aggregate, members: [...aggregate.members, evidence.successor_transcript_row_id],
          opening_evidence: { ...aggregate.opening_evidence, [evidence.successor_transcript_row_id]: Object.freeze({ ...evidence }) } };
        record('browser-authority.jsonl', { outcome: 'independent_committed_opening', evidence, source, reservation_epoch: reservation.epoch });
        for (const state of pending()) if (state.candidate) { state.committed_evidence_source = structuredClone(source); state.commit?.(); }
      } catch (error) { terminatePending('independent_evidence_mismatch'); throw error; }
    },
    registerKnownOpening(evidence) {
      assert.ok(ordinal === 0 && !sealed && !reservation, 'known facts must precede all requests and reservations');
      assert.ok(EDGE_FIELDS.every(k => typeof evidence[k] === 'string' && evidence[k].length > 0));
      assert.equal(evidence.product_conversation_id, aggregate.manual_opening.edge.product_conversation_id);
      assert.equal(evidence.predecessor_transcript_row_id, aggregate.members.at(-1));
      assert.ok(typeof evidence.client_turn_key === 'string' && evidence.client_turn_key.length > 0);
      assert.ok(!aggregate.members.includes(evidence.successor_transcript_row_id));
      aggregate = { ...aggregate, members: [...aggregate.members, evidence.successor_transcript_row_id],
        opening_evidence: { ...aggregate.opening_evidence, [evidence.successor_transcript_row_id]: { ...evidence } } };
      record('browser-authority.jsonl', { outcome: 'explicit_known_fact_not_held_gate_evidence', evidence });
    },
    sealAndDrain() {
      if (drain) return drain;
      sealed = true;
      drainDeadline = clock.now() + deadlineMs;
      page.off('request', onRequest);
      drainTimer = clock.setTimeout(() => terminatePending('deadline'), deadlineMs);
      drain = (async () => {
        while (pending().length || tasks.size) await new Promise(resolveWake => { wake = resolveWake; });
        clock.clearTimeout(drainTimer);
        page.off('response', onResponse); page.off('requestfailed', onFailed);
        drainComplete = true;
        const summary = inventory(false);
        record('browser-capture-drain.jsonl', summary);
        return summary;
      })();
      return drain;
    },
    assertQualified() {
      assert.ok(sealed && drainComplete && tasks.size === 0 && pending().length === 0, 'sealed completed drain required');
      assert.deepEqual(applicationErrors, [], 'application page errors');
      assert.deepEqual(captureErrors, [], 'required API/CDP capture errors');
      assert.ok([...required.values()].every(s => s.outcome === 'captured'
        || (s.outcome === 'request_failed' && s.expected_transport_failure === true)), 'all required outcomes captured or exact expected transport failure');
    },
  };
}

export const GLOBAL_PREFIX_ERROR_TEXT = 'Could not load earlier history:';

// Receipts are already validated by the installed recorder; revalidate the frozen selection.
export function selectGlobalPrefixFault(a, snapshots, mountedUrl, latest) {
  assert.equal(a.kind, 'global');
  const mounted = new URL(mountedUrl);
  const route = mounted.pathname.match(/^\/global\/([^/]+)$/);
  assert.ok(route, 'mounted Global route required');
  const pins = mounted.searchParams.getAll('source_transcript');
  assert.equal(pins.length, 1, 'unambiguous mounted source required');
  const source = pins[0];
  assert.ok(a.members.includes(decodeURIComponent(route[1])), 'registered mounted route required');
  assert.ok(a.members.includes(source), 'registered source alias required');
  const path = `/api/global/coordinator/history/${encodeURIComponent(source)}`;
  const baseline = snapshots.findLast(s => new URL(s.url).pathname === path);
  assert.ok(baseline, 'positive mounted history baseline required');
  assert.equal(baseline.method, 'GET'); assert.equal(baseline.status, 200);
  assert.match(baseline.type, /application\/json/);
  assert.notEqual(baseline.truncated, true);
  assert.equal(typeof baseline.raw, 'string');
  assert.ok(Buffer.byteLength(baseline.raw) <= REQUIRED_BODY_MAX_BYTES);
  const data = JSON.parse(baseline.raw);
  if (baseline.body_bytes !== undefined) assert.equal(baseline.body_bytes, Buffer.byteLength(baseline.raw));
  validateOwnedAggregate(a, data, baseline.url);
  assert.equal(data.requested_transcript_row_id, source);
  assert.equal(data.latest_transcript_row_id, latest);
  assert.equal(data.has_older, true); assert.ok(typeof data.before === 'string' && data.before.length > 0);
  return Object.freeze({ kind: 'global_history_prefix_acquisition', pathname: path,
    source_alias: source, product_conversation_id: data.product_conversation_id,
    latest_transcript_row_id: latest, positive_baseline: { url: baseline.url,
      status: baseline.status, request_ordinal: baseline.request_ordinal,
      before_parameter: new URL(baseline.url).searchParams.get('before'), next_before: data.before },
    observation_scope: 'earlier-history acquisition failure; not full fresh-tail refresh' });
}

async function run() {
  const app = resolve(process.env.APP_ROOT), output = resolve(process.env.HISTORY_OUTPUT);
  const ui = loopback(process.env.HISTORY_BASE_URL), api = loopback(process.env.HISTORY_API_URL);
  const a = JSON.parse(readFileSync(process.env.HISTORY_FIXTURE, 'utf8'));
  const fault = ownedFile(process.env.HISTORY_FAULT), control = process.env.HISTORY_CONTROL;
  const result = { status: 'failed', evidence_class: 'instrumented_runtime_not_shipping_proof', checks: [],
    uncovered: ['Generated authority cannot be selected by pinned manual POST; not qualified.', 'Shipping binary/providers/production/physical devices not exercised.'] };
  let records = 0;
  const record = (file, value) => { assert.ok(++records < 15000, 'receipt bound'); appendFileSync(resolve(output, file), JSON.stringify(value) + '\n'); };
  const checked = (name, data) => { result.checks.push(name); record('assertions.jsonl', { name, ...data }); };
  const gate = { armed: false, releaseAttempted: false, predecessor: a.members.at(-1), key: randomUUID() };
  const command = async body => { const reply = await ipc(control, body); record('ipc.jsonl', { command: body, reply }); return reply; };
  let browser, page, continuation, recorder;
  const snapshots = [], events = [];
  let requests = [], failed = [];
  const hardStop = setTimeout(() => {
    const capture = recorder?.partialInventory() ?? { partial: true, pending_metadata: [] };
    appendFileSync(resolve(output, 'browser-capture-drain.jsonl'), JSON.stringify(capture) + '\n');
    writeFileSync(resolve(output, 'browser-result.json'), JSON.stringify({ ...result, status: 'failed', partial: true, capture, error: 'wall deadline' }));
    process.exit(1);
  }, 170000);
  async function poll(fn, label, ms = 10000) {
    const end = performance.now() + ms;
    for (;;) { const value = await fn(); if (value) return value; assert.ok(performance.now() < end, label); await new Promise(r => setTimeout(r, 40)); }
  }
  async function http(method, path, body) {
    const response = await fetch(api + path, { method, headers: body ? { 'Content-Type': 'application/json' } : {},
      body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.timeout(10000) });
    const raw = await response.text(); record('driver-http.jsonl', { method, path, body, status: response.status, raw });
    assert.ok(response.ok, `${method} ${path}: ${response.status}`); return JSON.parse(raw);
  }
  async function anchor(key) {
    return page.evaluate(key => {
      const scroller = document.querySelector('#messages'); if (!scroller) return null;
      const viewport = scroller.getBoundingClientRect();
      const row = [...scroller.querySelectorAll('[data-virtual-key]')].find(el => {
        const box = el.getBoundingClientRect(); return (key ? el.dataset.virtualKey === key : box.top >= viewport.top)
          && box.height > 0 && box.bottom > viewport.top && box.top < viewport.bottom;
      });
      return row ? { key: row.dataset.virtualKey, offset: row.getBoundingClientRect().top - viewport.top } : null;
    }, key);
  }
  async function sameAnchor(before, label) {
    let frames = 0;
    const after = await poll(async () => { await page.evaluate(() => new Promise(r => requestAnimationFrame(r)));
      const value = await anchor(before.key); frames = anchorMatches(before, value) ? frames + 1 : 0; return frames >= 4 && value; }, label);
    checked(label, { before, after, tolerance_css_px: 2 });
  }
  const composerReadonly = async () => {
    const composer = page.locator(COMPOSER_SELECTOR);
    return await composer.count() === 0 || !await composer.isEditable();
  };
  try {
    const { chromium } = createRequire(resolve(app, 'ui/package.json'))('playwright');
    browser = await chromium.launch({ headless: true });
    const context = await browser.newContext({ viewport: { width: 1360, height: 900 } });
    page = await context.newPage(); page.setDefaultTimeout(10000); page.setDefaultNavigationTimeout(10000);
    recorder = installAggregateRecorder(page, a, record, snapshots, {
      expectedRequestFailure: entry => faultTarget(entry.method, entry.url, JSON.parse(readFileSync(fault)), 'roster'),
    });
    ({ requests, failed } = recorder);
    const streamRequests = new Map();
    const cdp = await context.newCDPSession(page); await cdp.send('Network.enable');
    cdp.on('Network.requestWillBeSent', e => { streamRequests.set(e.requestId, e.request.url); });
    cdp.on('Network.eventSourceMessageReceived', event => { events.push(event); record('browser-sse.jsonl', event); });

    // Verify the already-accepted manual edge independently; identical payload is not suppression authority.
    const previous = await http('GET', a.endpoint + '?boundary_representation=original_messages');
    validateOwnedAggregate(a, previous, api + a.endpoint + '?boundary_representation=original_messages');
    assert.equal(previous.latest_opening.status, 'accepted');
    assert.equal(previous.latest_opening.opening_authority, 'user_authorized_instruction');
    assert.equal(previous.latest_opening.presentation, 'separate_message');
    const oldEdge = previous.latest_opening.edge;
    const boundary = aggregateBoundaries(previous).find(b => b.message_id === oldEdge.continuation_message_id);
    const openingId = previous.latest_opening.accepted_message_id;
    assert.ok(boundary); assert.equal(boundary.represented_successor_opening_message_id, null);
    const oldRow = await http('GET', `/api/conversations/${gate.predecessor}`);
    const openingMessage = oldRow.messages.find(m => m.message_id === openingId);
    assert.ok(openingMessage); assert.equal(openingMessage.message_type, 'user');
    const predecessor = await http('GET', `/api/conversations/${oldEdge.predecessor_transcript_row_id}`);
    const boundaryMessage = predecessor.messages.find(m => m.message_id === oldEdge.continuation_message_id);
    assert.ok(boundaryMessage); assert.equal(boundaryMessage.content.summary, openingMessage.content.text);
    await page.goto(ui + sourceLink(a, gate.predecessor, openingId), { waitUntil: 'domcontentloaded' });
    await page.locator(persistedMessageSelector(openingId)).first().waitFor({ state: 'visible' });
    checked('manual equal-text accepted opening remains separate', { edge: oldEdge, opening_id: openingId, boundary_id: oldEdge.continuation_message_id });

    assert.equal((await command({ command: 'arm', predecessor: gate.predecessor, client_turn_key: gate.key })).state, 'armed'); gate.armed = true;
    recorder.reserveOpening({ kind: 'expected_manual_pending', product_conversation_id: a.manual_opening.edge.product_conversation_id,
      predecessor_transcript_row_id: gate.predecessor, client_turn_key: gate.key, opening_authority: 'user_authorized_instruction' });
    // POST acknowledgement may precede message persistence; only Entered certifies committed Pending.
    continuation = http('POST', `/api/conversations/${gate.predecessor}/continue`, { handoff: 'opening-probe [[perf:1]] [[ttft:0]]', message_id: gate.key });
    continuation.catch(() => {});
    const entered = await poll(async () => { const reply = await command({ command: 'entered' }); return reply.state === 'entered' && reply.evidence; }, 'committed Pending IPC', 10000);
    assert.equal(entered.predecessor_transcript_row_id, gate.predecessor); assert.equal(entered.client_turn_key, gate.key);
    assert.equal(entered.product_conversation_id, a.manual_opening.edge.product_conversation_id);
    assert.equal(entered.edge, 'committed_intent_before_first_message_persistence');
    const latest = entered.successor_transcript_row_id, acceptedId = `${latest}:${gate.key}`;
    recorder.registerCommittedOpening(entered, { kind: 'owned_ipc', command: 'entered', evidence: entered });
    const pending = await http('GET', a.endpoint + '?boundary_representation=original_messages');
    validateOwnedAggregate({ ...a, members: [...a.members, latest], opening_evidence: { [latest]: entered } }, pending, api + a.endpoint + '?boundary_representation=original_messages');
    assert.equal(pending.latest_transcript_row_id, latest); assert.ok(openingMatches(pending.latest_opening, entered, 'pending'));
    const direct = await http('GET', `/api/conversations/${latest}`);
    assert.equal(direct.messages.some(m => [gate.key, acceptedId].includes(m.message_id)), false);
    // New independent actual subscription must get Init while the write is held.
    const stream = await fetch(api + `/api/conversations/${latest}/stream`, { signal: AbortSignal.timeout(5000) });
    assert.equal(stream.status, 200); assert.match(stream.headers.get('content-type'), /text\/event-stream/);
    const reader = stream.body.getReader(); let raw = '';
    try { while (!raw.includes('\n\n')) { const value = await reader.read(); assert.equal(value.done, false); raw += new TextDecoder().decode(value.value); assert.ok(raw.length < 1000000); }
      record('independent-sse-init.jsonl', { latest, raw });
      const frame = raw.split('\n\n').find(frame => frame.includes('event: init'));
      assert.ok(frame); const init = JSON.parse(frame.split('\n').filter(line => line.startsWith('data:')).map(line => line.slice(5).trim()).join('\n'));
      assert.ok(openingMatches(init.opening, entered, 'pending'));
    } finally { await reader.cancel(); }
    updateFault(fault, { streamPath: `/api/conversations/${latest}/stream` });
    const start = snapshots.length;
    const middle = a.occurrences.filter(k => k[0] === 1)[20][2];
    await page.goto(ui + sourceLink(a, a.members[1], middle), { waitUntil: 'domcontentloaded' });
    await poll(() => snapshots.slice(start).find(s => belongsToAggregate(a, s) && s.data.latest_transcript_row_id === latest && openingMatches(s.data.latest_opening, entered, 'pending')), 'browser Pending aggregate');
    await page.locator(persistedMessageSelector(middle)).first().waitFor({ state: 'visible' });
    await poll(composerReadonly, 'Pending composer readonly');
    assert.equal(await page.locator(persistedMessageSelector(acceptedId)).count(), 0);
    let globalFaultObservation;
    const pin = new URL(page.url()).searchParams.get('source_transcript'); assert.equal(pin, a.members[1]);
    if (a.kind === 'global') {
      const owned = { ...a, members: [...a.members, latest], opening_evidence: { [latest]: entered } };
      const target = selectGlobalPrefixFault(owned, snapshots.slice(start), page.url(), latest);
      const prefixDeadline = performance.now() + 20000;
      const prefixBudget = () => { const left = Math.floor(prefixDeadline - performance.now()); assert.ok(left > 0, 'Global prefix action absolute 20s bound'); return Math.min(5000, left); };
      const savedTop = await page.locator('#messages').evaluate(el => el.scrollTop);
      const pendingComposerCount = await page.locator(COMPOSER_SELECTOR).count();
      const failureStart = failed.length;
      updateFault(fault, { aggregatePath: target.pathname, failRoster: true });
      // A DOM scroll uses the rendered transcript's prefix-top sentinel, not React internals.
      await page.locator('#messages').evaluate(el => { el.scrollTop = 0; });
      const request = await poll(() => failed.slice(failureStart).find(r =>
        faultTarget(r.method, r.url, JSON.parse(readFileSync(fault)), 'roster')
        && new URL(r.url).searchParams.get('message_limit') === '100'
        && !!new URL(r.url).searchParams.get('before')), 'actual failed Global prefix GET', prefixBudget());
      assert.equal(new URL(request.url).searchParams.get('before'), target.positive_baseline.next_before, 'actual prefix cursor from positive mounted page');
      const transport = readFileSync(resolve(output, 'preview-transport.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
      const destroyed = transport.find(t => t.action === 'destroy-roster' && t.method === 'GET'
        && new URL(t.url, ui).pathname === target.pathname
        && new URL(t.url, ui).search === new URL(request.url).search);
      assert.ok(destroyed, 'owned preview destroyed the actual prefix GET before release');
      const prefixError = page.locator('#chat-view [role="alert"]').filter({ hasText: GLOBAL_PREFIX_ERROR_TEXT });
      await prefixError.waitFor({ state: 'visible', timeout: prefixBudget() });
      assert.equal(await page.locator(COMPOSER_SELECTOR).count(), pendingComposerCount, 'prefix failure does not erase Pending composer');
      assert.equal(await composerReadonly(), true, 'prefix failure retains Pending composer');
      assert.equal((await command({ command: 'entered' })).state, 'entered');
      assert.equal(gate.releaseAttempted, false);
      assert.equal(new URL(page.url()).searchParams.get('source_transcript'), pin);
      globalFaultObservation = { ...target, request, destroyed,
        before_parameter: new URL(request.url).searchParams.get('before'), failed_while_pending: true };
      checked('Global earlier-history acquisition failed while Pending before release', globalFaultObservation);
      await page.locator('#messages').evaluate((el, top) => { el.scrollTop = top; }, savedTop);
      await page.locator(persistedMessageSelector(middle)).first().waitFor({ state: 'visible', timeout: prefixBudget() });
      await page.locator(persistedMessageSelector(middle)).first().evaluate(el => el.scrollIntoView({ block: 'center' }));
      await prefixError.waitFor({ state: 'visible', timeout: prefixBudget() });
      prefixBudget();
    }
    const before = await poll(() => anchor(), 'historical physical anchor');
    assert.equal(requests.some(r => r.method === 'POST' && new URL(r.url).pathname.endsWith('/chat')), false);
    checked('committed Pending: browser/HTTP/new SSE init available, opening deferred and composer readonly', { entered, before });
    if (a.kind !== 'global') {
      const actualRoster = snapshots.slice(start).find(s => belongsToAggregate(a, s));
      updateFault(fault, { aggregatePath: new URL(actualRoster.url).pathname, failRoster: true });
    }
    const acceptedStart = events.length, failureStart = failed.length;
    await releaseOnce(gate, command);
    const continued = await continuation; assert.equal(continued.status, 'accepted'); assert.equal(continued.conversation_id, latest);
    const accepted = await poll(() => events.slice(acceptedStart).find(e => {
      const value = JSON.parse(e.data); return ['message', 'message_updated'].includes(e.eventName) && openingMatches(value.opening, entered, 'accepted');
    }), 'actual accepted opening SSE', 10000);
    const event = JSON.parse(accepted.data); const acceptedCursor = acceptedWireCursor(accepted, events);
    assert.equal(event.message?.message_id ?? event.message_id, acceptedId);
    const failedRoster = globalFaultObservation?.request ?? await poll(() => failed.slice(failureStart).find(r => faultTarget(r.method, r.url, JSON.parse(readFileSync(fault)), 'roster')), 'actual failed owned roster GET');
    const failedRosterUrl = new URL(failedRoster.url);
    const faultObservation = globalFaultObservation ?? { request: failedRoster, pathname: failedRosterUrl.pathname,
      message_limit: failedRosterUrl.searchParams.get('message_limit'),
      observation_scope: failedRosterUrl.searchParams.get('message_limit') === '1'
        ? 'background aggregate ownership probe; no page-alert contract'
        : 'aggregate acquisition; caller-specific error display not assumed' };
    await poll(async () => await page.locator(COMPOSER_SELECTOR).isEditable(), 'Accepted composer enabled despite roster failure');
    await sameAnchor(before, 'acceptance retains physical anchor');
    assert.equal(new URL(page.url()).searchParams.get('source_transcript'), pin);
    const text = `composer-${randomUUID()} [[perf:1]] [[ttft:0]]`;
    const posted = page.waitForRequest(req => req.method() === 'POST' && new URL(req.url()).pathname.endsWith('/chat'));
    await page.locator(COMPOSER_SELECTOR).fill(text); await page.locator(COMPOSER_SELECTOR).press('Enter');
    const req = await posted; assert.equal(new URL(req.url()).pathname, `/api/conversations/${latest}/chat`);
    const body = req.postDataJSON(); assert.equal(body.text, text);
    const ack = await req.response(); assert.ok(ack?.ok()); const ackRaw = await ack.text();
    const chatAck = { url: ack.url(), status: ack.status(), raw: ackRaw, body: JSON.parse(ackRaw),
      canonical_message_id: `${latest}:${body.message_id}` };
    checked('actual successor chat ACK receipt', chatAck);
    validateChatAck(chatAck, latest, body.message_id);
    await poll(async () => (await http('GET', `/api/conversations/${latest}`)).messages.some(m => m.message_id === `${latest}:${body.message_id}`), 'actual composer message persisted');
    checked('accepted SSE unblocks exact successor composer despite actual aggregate GET failure', { accepted_cursor: acceptedCursor, fault: faultObservation, chat_message_id: body.message_id });
    const reconnectStart = events.length, requestStart = requests.length, url = page.url();
    updateFault(fault, { drop: 1 });
    await poll(() => requests.slice(requestStart).find(r => faultTarget(r.method, r.url, JSON.parse(readFileSync(fault)), 'stream')), 'same-page actual SSE reconnect request', 15000);
    const reconnect = await poll(() => events.slice(reconnectStart).find(e => e.requestId !== accepted.requestId && streamRequests.has(e.requestId) && faultTarget('GET', streamRequests.get(e.requestId), JSON.parse(readFileSync(fault)), 'stream') && e.eventName === 'init' && openingMatches(JSON.parse(e.data).opening, entered, 'accepted')), 'same-page accepted reconnect Init', 15000);
    const transport = readFileSync(resolve(output, 'preview-transport.jsonl'), 'utf8').trim().split('\n').map(line => JSON.parse(line));
    assert.ok(transport.some(t => t.action === 'drop-streams' && t.token === 1 && t.count > 0), 'owned preview actually destroyed live SSE responses');
    assert.equal(page.url(), url); assert.equal(new URL(page.url()).searchParams.get('source_transcript'), pin);
    await poll(async () => await page.locator(COMPOSER_SELECTOR).isEditable(), 'reconnected composer stays enabled');
    await sameAnchor(before, 'reconnect retains physical anchor');
    // Exact identity, not text heuristics: at most one occurrence in aggregate DOM/navigation.
    assert.ok(await page.locator(persistedMessageSelector(acceptedId)).count() <= 1);
    const finalRow = await http('GET', `/api/conversations/${latest}`);
    assert.equal(finalRow.messages.filter(m => m.message_id === acceptedId).length, 1);
    checked('same-page transport SSE drop/reconnect preserves accepted full-edge identity', { accepted_cursor: acceptedCursor, reconnect, requests: requests.slice(requestStart) });
    await page.screenshot({ path: resolve(output, 'accepted-reconnected.png') });
    updateFault(fault, { failRoster: false });
    const restoredAggregate = page.waitForResponse(response => response.request().method() === 'GET'
      && capturePolicy({ ...a, members: [...a.members, latest] }, 'GET', response.url()).kind === 'required_aggregate_json'
      && response.ok());
    // An explicit chapter selection may move the reader; verify its persisted ID after the click.
    const chapter = page.locator('#conversation-nav [role="button"]').filter({ hasText: 'opening-probe' });
    assert.equal(await chapter.count(), 1); await chapter.click();
    const restoredResponse = await restoredAggregate;
    const fetchedLatestAccepted = JSON.parse(await restoredResponse.text());
    validateOwnedAggregate({ ...a, members: [...a.members, latest], opening_evidence: { [latest]: entered } }, fetchedLatestAccepted, restoredResponse.url());
    assert.ok(openingMatches(fetchedLatestAccepted.latest_opening, entered, 'accepted'), 'restored full accepted edge');
    checked('new chapter ownership acquisition uses restored real aggregate transport', {
      url: restoredResponse.url(), status: restoredResponse.status(), fault_active_during_acceptance_and_reconnect: true,
      fault_restored_before_new_pin_acquisition: true });
    await page.waitForURL(value => value.hash === `#message-${encodeURIComponent(acceptedId)}`);
    const rendered = page.locator(persistedMessageSelector(acceptedId));
    await rendered.waitFor({ state: 'visible' }); assert.equal(await rendered.count(), 1);
    const physical = await rendered.evaluate(el => { const box = el.getBoundingClientRect(); const view = document.querySelector('#messages').getBoundingClientRect(); return box.height > 0 && box.bottom > view.top && box.top < view.bottom; });
    assert.equal(physical, true);
    checked('accepted manual opening renders once with exact persisted identity after real chapter click', { acceptedId });
    await recorder.sealAndDrain(); recorder.assertQualified();
    result.status = 'passed_assertions';
  } catch (error) {
    result.error = String(error.stack ?? error); process.exitCode = 1;
    if (page) await page.screenshot({ path: resolve(output, 'failure.png') }).catch(() => {});
  } finally {
    try { if (recorder) { await recorder.sealAndDrain(); recorder.assertQualified(); } }
    catch (error) { result.status = 'failed'; result.capture_error = String(error); process.exitCode = 1; }
    try { if (gate.armed && !gate.releaseAttempted) {
      const entered = await command({ command: 'entered' });
      if (entered.state === 'entered') await releaseOnce(gate, command);
    } } catch (error) { record('cleanup-errors.jsonl', { error: String(error) }); process.exitCode = 1; result.status = 'failed'; }
    try { await command({ command: 'stop' }); } catch (error) { record('cleanup-errors.jsonl', { error: String(error) }); process.exitCode = 1; result.status = 'failed'; }
    try { updateFault(fault, { failRoster: false }); if (browser) await browser.close(); }
    catch (error) { result.status = 'failed'; result.cleanup_error = String(error); process.exitCode = 1; }
    finally {
      try { recorder?.assertQualified(); }
      catch (error) { result.status = 'failed'; result.capture_error = String(error); process.exitCode = 1; }
      result.application_page_errors = recorder?.applicationErrors ?? [];
      result.cdp_capture_errors = recorder?.captureErrors ?? [];
      clearTimeout(hardStop); writeFileSync(resolve(output, 'browser-result.json'), JSON.stringify(result, null, 2) + '\n');
    }
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) await run();
