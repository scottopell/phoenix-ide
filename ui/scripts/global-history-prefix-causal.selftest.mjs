/** Offline causal sequences, not captured browser gestures or runtime qualification. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { stripTypeScriptTypes } from 'node:module';
import { readJsonLines, retainedResponse } from './replay-history-opening-recorder.mjs';
import { selectGlobalPrefixFault, globalPrefixWheelDelta } from './qualify-history-opening-realserver.mjs';
import { APP_PIN } from './history-joint-telemetry.mjs';
const root = process.env.HISTORY_PINNED_SOURCE;
const dir = process.env.HISTORY_GLOBAL_B1_EVIDENCE;
const sha = s => createHash('sha256').update(s).digest('hex');
const load = async s => import('data:text/javascript;base64,' + Buffer.from(stripTypeScriptTypes(s)).toString('base64'));
let replay;
test('pinned actual reducer + extracted actual reader predicate/request guard: offline DOM vs hardware event sequence', { skip: !root || !dir }, async () => {
  const pin = APP_PIN;
  const retainedSourcePin = 'd4c2772a95aa46da608b356c8fe7be074de1b7aa';
  assert.equal(execFileSync('git', ['-C', root, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(), pin);
  const files = ['ui/src/conversation/scrollMachine.ts', 'ui/src/components/MessageList.tsx'];
  const sources = files.map(path => { const source = readFileSync(`${root}/${path}`, 'utf8');
    assert.equal(source, execFileSync('git', ['-C', root, 'show', `${pin}:${path}`], { encoding: 'utf8', maxBuffer: 1024 * 1024 })); return source; });
  assert.doesNotMatch(sources[0], /^import\s/m);
  const reducer = await load(sources[0]);
  const predicate = sources[1].match(/function readerMovedViewport\([\s\S]*?\n}/)?.[0]; assert.ok(predicate);
  const guard = sources[1].match(/const ownsCurrentView = machine.kind === 'live'[\s\S]*?\) return;/)?.[0]; assert.ok(guard);
  // The actual guard's return terminates this pure probe; no fake load callback is invoked.
  const probe = await load(`export ${predicate}\nexport function eligible(machine, conversationId, source, earlierHistoryRequestScheduledRef, hasOlderMessages, loadingOlderMessages, onLoadOlderMessages, olderHistoryError) { ${guard} return true; }`);
  const rows = readJsonLines(`${dir}/browser-responses.jsonl`).filter(r => r.value.outcome === 'body_received' && [42,95,100].includes(r.value.request_ordinal));
  assert.equal(rows.length, 3);
  const bodies = rows.map(row => { const retained = retainedResponse(row); return { ...row.value, ...retained, data: JSON.parse(retained.raw) }; });
  assert.deepEqual(bodies.map(b => b.data.segments.map(s => s.messages.length)), [[0,20,76,4],[0,20,76,4,0],[44,56,0,0,0]]);
  for (const body of bodies) { assert.equal(body.data.has_older, true); assert.ok(body.data.before); }
  const a = JSON.parse(readFileSync(`${dir}/fixture.json`));
  const entered = readJsonLines(`${dir}/ipc.jsonl`).find(r => r.value.reply.evidence)?.value.reply.evidence; assert.ok(entered);
  a.members.push(entered.successor_transcript_row_id); a.opening_evidence = { [entered.successor_transcript_row_id]: entered };
  const mounted = `http://127.0.0.1:1${a.route}?source_transcript=${a.members[1]}`;
  const target = selectGlobalPrefixFault(a, bodies, mounted, entered.successor_transcript_row_id);
  assert.equal(target.positive_baseline.request_ordinal, 100);
  const id = bodies[2].data.product_conversation_id, snapshot = { scrollTop: 0, scrollHeight: 10000, clientHeight: 700 };
  const events = [{ type: 'navigationJumped' }, { type: 'conversationMeasured', conversationId: id, snapshot, totalHeight: 10000, unitCount: 100, nowMs: 0 }];
  let state = reducer.initialScrollMachineState(id);
  for (const event of events) state = reducer.reduceScrollMachine(state, event).state;
  assert.deepEqual(state.follow, { kind: 'navigating', phase: 'positioning' });
  const eligible = (machine, changes = {}) => probe.eligible(machine, changes.conversationId ?? id, 'range', { current: changes.scheduled ?? false }, changes.hasOlder ?? true, changes.loading ?? false, true, changes.error ?? null) === true;
  const old = reducer.reduceScrollMachine(state, { type: 'scrollerAttached', snapshot }).state;
  assert.equal(probe.readerMovedViewport(old), false); assert.equal(eligible(old), false);
  // onWheel dispatch order is extracted and checked, not inferred from upwardIntent alone.
  const wheelSource = sources[1].match(/const onWheel = [\s\S]*?\n      };/)?.[0]; assert.ok(wheelSource);
  const retainedSources = files.map(path => execFileSync('git', ['-C', root, 'show', `${retainedSourcePin}:${path}`], { encoding: 'utf8', maxBuffer: 1024 * 1024 }));
  assert.equal(sources[0], retainedSources[0], 'candidate scroll reducer remains byte-identical to retained historical source');
  assert.equal(predicate, retainedSources[1].match(/function readerMovedViewport\([\s\S]*?\n}/)?.[0]);
  assert.equal(guard, retainedSources[1].match(/const ownsCurrentView = machine.kind === 'live'[\s\S]*?\) return;/)?.[0]);
  assert.equal(wheelSource, retainedSources[1].match(/const onWheel = [\s\S]*?\n      };/)?.[0]);
  const types = [...wheelSource.matchAll(/dispatchScrollEvent\(\{ type: '([^']+)'/g)].map(m => m[1]);
  assert.deepEqual(types, ['interactionStarted', 'upwardIntent']);
  assert.ok(wheelSource.includes("dispatchTranscriptPositioningRef.current({ type: 'user_interrupted' })"));
  const wheelEvents = types.map(type => type === 'upwardIntent' ? { type, snapshot } : { type });
  let wheel = state; for (const event of wheelEvents) wheel = reducer.reduceScrollMachine(wheel, event).state;
  assert.deepEqual(wheel.follow, { kind: 'navigating', phase: 'user-returning' });
  assert.equal(probe.readerMovedViewport(wheel), true); assert.equal(eligible(wheel), true);
  for (const changes of [{ loading: true }, { error: 'offline-error' }, { scheduled: true }, { hasOlder: false }, { conversationId: 'foreign' }]) assert.equal(eligible(wheel, changes), false);
  assert.equal(eligible(reducer.reduceScrollMachine(state, { type: 'upwardIntent', snapshot }).state), false);
  const requests = readJsonLines(`${dir}/browser-requests.jsonl`);
  for (const row of rows) assert.ok(requests.some(r => r.value.request_ordinal === row.value.request_ordinal && r.value.url === row.value.url && r.value.method === 'GET'));
  assert.equal(requests.some(r => r.value.request_ordinal > 100 && r.value.url.includes(target.pathname) && new URL(r.value.url).searchParams.has('before')), false);
  assert.equal(readFileSync(`${dir}/preview.log`, 'utf8').includes('destroy-roster'), false);
  const failures = readJsonLines(`${dir}/browser-failed.jsonl`);
  assert.equal(failures.some(r => r.value.url.includes(target.pathname) && new URL(r.value.url).searchParams.get('before') === bodies[2].data.before), false);
  replay = { evidence_class: 'OFFLINE_causal_synthetic_events_NOT_browser_cause_proof', runtime_executed: false,
    pin, retainedSourcePin, source_scope: 'candidate source replay against historical artifact, not new runtime acceptance', sources: files.map((path,i) => ({ path, sha256: sha(sources[i]) })), predicate_sha256: sha(predicate), guard_sha256: sha(guard),
    retained: rows.map(r => ({ source: r.source, request_ordinal: r.value.request_ordinal, raw_sha256: sha(r.value.raw), before: JSON.parse(r.value.raw).before })),
    target, synthetic_geometry: snapshot, events, old_positioning_state: old, wheel_events: wheelEvents, wheel_state: wheel,
    independent_opening_source: entered };
  if (process.env.HISTORY_PREFIX_CAUSAL_OUTPUT) writeFileSync(process.env.HISTORY_PREFIX_CAUSAL_OUTPUT, JSON.stringify(replay, null, 2) + '\n');
});
test('hardware wheel delta is finite negative and bounded by actual observed position', () => {
  assert.equal(globalPrefixWheelDelta({ scrollTop: 0 }), -64);
  assert.equal(globalPrefixWheelDelta({ scrollTop: 123.5 }), -188);
  assert.equal(globalPrefixWheelDelta({ scrollTop: 1e9 }), -16384);
  for (const scrollTop of [NaN, Infinity, -1, undefined]) assert.throws(() => globalPrefixWheelDelta({ scrollTop }));
});
test('runtime walk preserves first current-cursor fault, sentinel and gate ordering without programmatic trigger', () => {
  const source = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const action = source.slice(source.indexOf("    if (a.kind === 'global') {\n      const owned"), source.indexOf("    const before = a.kind === 'global'"));
  assert.ok(action.indexOf('settled Global viewport') < action.indexOf('const target = selectGlobalPrefixFault'));
  assert.ok(action.indexOf('failRoster: true') < action.indexOf('page.mouse.wheel'));
  assert.match(action, /attempt < 3/); assert.match(action, /post.scrollTop <= 2 && post.startIndex <= 2/);
  assert.match(action, /actual prefix cursor from positive mounted page/);
  assert.match(action, /gate.releaseAttempted, false/);
  assert.doesNotMatch(action.slice(0, action.indexOf("checked('Global earlier-history")), /scrollTop\s*=(?!=)|dispatchEvent|__react|onLoadOlderMessages/);
});
