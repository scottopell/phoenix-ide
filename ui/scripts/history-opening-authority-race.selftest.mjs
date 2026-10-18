import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { readFileSync } from 'node:fs';
import { installAggregateRecorder } from './qualify-history-opening-realserver.mjs';
import { readJsonLines, retainedResponse, replayReservationRace } from './replay-history-opening-recorder.mjs';
const root = process.env.HISTORY_RACE_EVIDENCE;
const flush = () => new Promise(resolve => setImmediate(resolve));
function clockAdapter() {
  let time = 0, id = 0;
  const timers = new Map();
  return { now: () => time, setTimeout(fn, delay) { timers.set(++id, { fn, at: time + delay }); return id; },
    clearTimeout(key) { timers.delete(key); }, advance(ms) {
      time += ms;
      for (const [key, timer] of [...timers]) if (timer.at <= time) { timers.delete(key); timer.fn(); }
    }, deadlines: () => [...timers.values()].map(t => t.at) };
}
for (const [artifact, kind, requestOrdinal] of [['40', 'ordinary', 51], ['5fb', 'global', 95]]) {
  test(`authentic ${kind} response-before-IPC, retained stamp and fail-closed evidence`, { skip: !root, timeout: 5000 }, async () => {
    const dir = `${root}/instrumented${artifact}-artifact/opening-browser-evidence/${kind}`;
    const a = JSON.parse(readFileSync(`${dir}/fixture.json`)); assert.equal(a.members.length, 4);
    const ipc = readJsonLines(`${dir}/ipc.jsonl`);
    const arm = ipc.find(r => r.value.command?.command === 'arm');
    const entered = ipc.find(r => r.value.reply?.state === 'entered' && r.value.reply.evidence);
    const bodyRows = readJsonLines(`${dir}/browser-responses.jsonl`).filter(r => r.value.outcome === 'body_received'
      && r.value.url.includes(kind === 'ordinary' ? '/api/product-conversations/' : '/api/global/coordinator/history/'));
    const row = bodyRows.find(r => r.value.request_ordinal === requestOrdinal); assert.ok(row);
    const r = retainedResponse(row), data = JSON.parse(r.raw); assert.equal(data.segments.length, 5);
    const replay = await replayReservationRace(a, bodyRows, ipc);
    assert.equal(replay.snapshots.length, bodyRows.length);
    const staged = replay.receipts.find(r => r.value.outcome === 'awaiting_independent_committed_evidence').value;
    assert.equal(staged.raw_request_stamp.retained_request_ordinal, requestOrdinal);
    assert.equal(staged.validation, 'structural_candidate');
    assert.equal(staged.reservation_epoch, 1);
    const captured = replay.snapshots.find(s => s.raw === r.raw);
    assert.equal(captured.request_ordinal, staged.request_ordinal);
    assert.equal(captured.raw_sha256, staged.raw_sha256);
    assert.equal(captured.committed_evidence_source.line_sha256, entered.source.line_sha256);
    function setup({ reserve = true, mutate, delayedBody = false } = {}) {
      const page = new EventEmitter(), snapshots = [], receipts = [], clock = clockAdapter(); page.mainFrame = () => page;
      const recorder = installAggregateRecorder(page, a, (file, value) => receipts.push({ file, value }), snapshots, { clock,
        requestStamp: () => ({ ...row.source, retained_request_ordinal: requestOrdinal }) });
      if (reserve) recorder.reserveOpening({ kind: 'expected_manual_pending', product_conversation_id: a.manual_opening.edge.product_conversation_id,
        predecessor_transcript_row_id: arm.value.command.predecessor, client_turn_key: arm.value.command.client_turn_key,
        opening_authority: 'user_authorized_instruction' });
      const req = { url: () => r.url, method: () => 'GET', postData: () => null };
      page.emit('request', req);
      const d = structuredClone(data); mutate?.(d);
      let complete;
      page.emit('response', { request: () => req, url: req.url, status: () => 200, headers: () => ({ 'content-type': r.type }),
        text: () => delayedBody ? new Promise(resolve => { complete = resolve; }) : Promise.resolve(mutate ? JSON.stringify(d) : r.raw) });
      const source = { kind: 'owned_ipc', command: 'entered', evidence: entered.value.reply.evidence, ...entered.source };
      return { recorder, snapshots, clock, receipts, source, complete: () => complete(r.raw) };
    }
    for (const [name, mutate] of Object.entries({ root: d => { d.canonical_root.transcript_row_id = 'foreign'; },
      product: d => { d.product_conversation_id = 'foreign'; }, unknownPrefix: d => { d.segments[1].transcript_row_id = 'unknown'; },
      extraTwo: d => { d.segments.push({ ...d.segments.at(-1), transcript_row_id: 'extra' }); },
      key: d => { d.latest_opening.client_turn_key = 'wrong'; }, authority: d => { d.latest_opening.opening_authority = 'generated_predecessor_context'; },
      accepted: d => { d.latest_opening.status = 'accepted'; }, projection: d => { kind === 'ordinary' ? d.segments[0].handoff = { kind: 'fiction' } : delete d.boundaries; } })) {
      const f = setup({ mutate }); await flush(); assert.equal(f.recorder.partialInventory().pending_required, 0, name);
      await f.recorder.sealAndDrain(); assert.throws(() => f.recorder.assertQualified(), /required API/); assert.equal(f.snapshots.length, 0);
    }
    const unreserved = setup({ reserve: false }); await flush(); await unreserved.recorder.sealAndDrain(); assert.throws(() => unreserved.recorder.assertQualified());
    for (const field of ['product_conversation_id', 'predecessor_transcript_row_id', 'successor_transcript_row_id', 'continuation_message_id', 'client_turn_key', 'edge']) {
      const f = setup(); await flush(); assert.equal(f.recorder.partialInventory().pending_metadata[0].outcome, 'awaiting_independent_committed_evidence');
      const wrong = { ...entered.value.reply.evidence, [field]: 'wrong' };
      assert.throws(() => f.recorder.registerCommittedOpening(wrong, { ...f.source, evidence: wrong }));
      await f.recorder.sealAndDrain(); assert.throws(() => f.recorder.assertQualified()); assert.equal(f.snapshots.length, 0);
    }
    for (const late of [false, true]) {
      const f = setup({ delayedBody: true }); await flush(); f.clock.advance(6000); f.complete(); await flush();
      assert.deepEqual(f.clock.deadlines(), [11000]); const drain = f.recorder.sealAndDrain();
      f.clock.advance(4999); assert.equal(f.recorder.partialInventory().pending_required, 1);
      if (late) f.clock.advance(1);
      if (late) assert.throws(() => f.recorder.registerCommittedOpening(entered.value.reply.evidence, f.source));
      else f.clock.advance(1); // absent IPC must also fail at original response deadline, not seal +11s
      await drain; assert.throws(() => f.recorder.assertQualified()); assert.equal(f.snapshots.length, 0);
    }
    const f = setup(); await flush(); assert.throws(() => f.recorder.registerKnownOpening(entered.value.reply.evidence));
    f.clock.advance(11000); await f.recorder.sealAndDrain(); assert.throws(() => f.recorder.assertQualified());
  });
}
