import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { replayInstalled, readJsonLines, retainedResponse, replayAcceptedEvents } from './replay-history-opening-recorder.mjs';
import { validateOwnedAggregate } from './qualify-history-opening-realserver.mjs';
const evidence = process.env.HISTORY_OFFLINE_EVIDENCE;
for (const kind of ['ordinary', 'global']) {
  test(`authentic ${kind} full retained byte producer + fail-closed mutations`, { skip: !evidence }, async () => {
    const dir = `${evidence}/${kind}`, a = JSON.parse(readFileSync(`${dir}/replay-fixture.json`));
    const provenance = JSON.parse(readFileSync(`${dir}/provenance.json`));
    const rows = provenance.map(p => readJsonLines(p.path)[p.line - 1]);
    const openingEvidence = a.replay_registered_opening_evidence;
    await replayInstalled(a, rows, { openingEvidence });
    const base = rows[0], raw = retainedResponse(base).raw;
    const mutations = {
      owner: d => { d.product_conversation_id = 'foreign'; },
      root: d => { d.canonical_root.transcript_row_id = 'foreign'; },
      member: d => { d.segments[0].transcript_row_id = 'foreign'; },
      ordinal: d => { d.segments[0].segment_ordinal = 8; },
      missing_segments: d => { delete d.segments; },
      selected: d => { d.requested_transcript_row_id = 'foreign'; },
      latest: d => { d.latest_transcript_row_id = d.segments[0].transcript_row_id; },
      opening_owner: d => { d.latest_opening.edge.product_conversation_id = 'foreign'; },
      opening_edge: d => { d.latest_opening.edge.continuation_message_id = 'foreign'; },
      opening_id: d => { d.latest_opening.accepted_message_id = 'wrong:key'; },
      authority: d => { d.latest_opening.opening_authority = 'generated_predecessor_context'; },
      presentation: d => { d.latest_opening.presentation = 'boundary'; },
    };
    for (const [name, mutate] of Object.entries(mutations)) {
      const d = JSON.parse(raw); mutate(d);
      await assert.rejects(replayInstalled(a, [{ ...base, value: { ...base.value, raw: JSON.stringify(d), body: undefined, response: undefined, body_bytes: undefined } }], { openingEvidence }), /required API\/CDP capture errors/, name);
    }
    const request = { ...base, value: { ...base.value, url: 'http://127.0.0.1:1/api/global/coordinator/history/foreign' } };
    await assert.rejects(replayInstalled(a, [request], { openingEvidence }), /owned required/, 'foreign request');
    assert.throws(() => retainedResponse({ ...base, value: { ...base.value, truncated: true } }), /truncated/);
    await assert.rejects(replayInstalled(a, [{ ...base, value: { ...base.value, raw: '{truncated', body_bytes: undefined } }], { openingEvidence }), /required API/);
    if (kind === 'ordinary') {
      const handoffRow = rows.find(r => JSON.parse(retainedResponse(r).raw).segments.some(s => s.handoff));
      assert.ok(handoffRow);
      for (const field of ['kind', 'predecessor_transcript_row_id', 'successor_transcript_row_id', 'continuation_message_id', 'accepted_successor_message_id', 'opening_authority']) {
        const d = JSON.parse(retainedResponse(handoffRow).raw), h = d.segments.find(s => s.handoff?.kind === 'completed').handoff; h[field] = 'foreign';
        await assert.rejects(replayInstalled(a, [{ ...handoffRow, value: { ...handoffRow.value, raw: JSON.stringify(d), body_bytes: undefined } }], { openingEvidence }), /required API/, `handoff ${field}`);
      }
    } else {
      // Explicit synthetic mutation only: not a real Global Pending or held gate.
      const d = JSON.parse(raw), edge = d.latest_opening.edge;
      const client_turn_key = d.latest_opening.accepted_message_id.slice(edge.successor_transcript_row_id.length + 1);
      d.latest_opening = { status: 'pending', edge, client_turn_key, opening_authority: 'user_authorized_instruction' };
      const aPending = { ...a, opening_evidence: { [edge.successor_transcript_row_id]: { ...edge, client_turn_key } } };
      validateOwnedAggregate(aPending, d, retainedResponse(base).url);
      d.latest_opening.client_turn_key = 'wrong'; assert.throws(() => validateOwnedAggregate(aPending, d, retainedResponse(base).url));
    }
  });
}
test('authentic SSE owning request/sequence failures reject; Global messages are not relabeled acceptance', { skip: !evidence }, () => {
  for (const kind of ['ordinary', 'global']) {
    const manifest = JSON.parse(readFileSync(`${evidence}/${kind}/manifest.json`));
    const path = Object.keys(manifest.input_hashes).find(p => p.endsWith('browser-sse.jsonl'));
    const a = JSON.parse(readFileSync(`${evidence}/${kind}/replay-fixture.json`));
    const rows = readJsonLines(path), result = replayAcceptedEvents(rows, a.product_conversation_id);
    assert.equal(result.acceptance_transition_observed, kind === 'ordinary');
    const cursor = [...result.cursors, ...result.postAcceptedMessages][0];
    const wrongRequest = structuredClone(rows); wrongRequest[cursor.line - 1].value.requestId = 'foreign';
    assert.throws(() => replayAcceptedEvents(wrongRequest, a.product_conversation_id));
    const wrongSeq = structuredClone(rows), event = wrongSeq[cursor.line - 1].value, d = JSON.parse(event.data); d.sequence_id = -1; event.data = JSON.stringify(d);
    assert.throws(() => replayAcceptedEvents(wrongSeq, a.product_conversation_id));
  }
});
