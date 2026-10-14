import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { selectGlobalPrefixFault, faultTarget, GLOBAL_PREFIX_ERROR_TEXT } from './qualify-history-opening-realserver.mjs';
const dir = process.env.HISTORY_GLOBAL_5FB_EVIDENCE;
const sourceRoot = process.env.HISTORY_PINNED_SOURCE;
const lines = name => readFileSync(`${dir}/${name}`, 'utf8').trim().split('\n').map(JSON.parse);
test('authentic 5fb Global: latest mounted positive page, not original route or 404 ownership probe', { skip: !dir }, () => {
  const a = JSON.parse(readFileSync(`${dir}/fixture.json`));
  const entered = lines('ipc.jsonl').find(r => r.reply.evidence)?.reply.evidence;
  assert.ok(entered); a.members.push(entered.successor_transcript_row_id);
  a.opening_evidence = { [entered.successor_transcript_row_id]: entered };
  const responses = lines('browser-responses.jsonl'), requests = lines('browser-requests.jsonl');
  const snapshots = responses.filter(r => r.outcome === 'body_received').map(r => ({ ...r, data: JSON.parse(r.raw) }));
  const source = a.members[1], latest = entered.successor_transcript_row_id;
  const mounted = `http://127.0.0.1:1${a.route}?source_transcript=${source}`;
  const selected = selectGlobalPrefixFault(a, snapshots, mounted, latest);
  assert.equal(selected.kind, 'global_history_prefix_acquisition');
  assert.equal(selected.source_alias, source);
  assert.equal(selected.positive_baseline.request_ordinal, 100);
  assert.equal(selected.positive_baseline.status, 200);
  assert.ok(selected.positive_baseline.before_parameter);
  assert.notEqual(selected.pathname, new URL(snapshots[0].url).pathname);
  assert.ok(Object.isFrozen(selected));
  for (const r of snapshots) assert.ok(requests.some(q => q.request_ordinal === r.request_ordinal && q.url === r.url && q.method === 'GET'));
  const negative = responses.find(r => r.status === 404 && r.url.includes(latest));
  assert.ok(negative); assert.equal(faultTarget('GET', negative.url, { failRoster: true, aggregatePath: selected.pathname }, 'roster'), false);
  assert.throws(() => selectGlobalPrefixFault(a, [negative], mounted, latest));
  const baseline = snapshots.at(-1);
  for (const change of [{ status: 404 }, { truncated: true }, { raw: baseline.raw.slice(0, -1) }, { body_bytes: 1 }, { method: 'POST' }]) {
    assert.throws(() => selectGlobalPrefixFault(a, [...snapshots, { ...baseline, ...change }], mounted, latest));
  }
  const foreign = JSON.parse(baseline.raw); foreign.product_conversation_id = 'foreign';
  assert.throws(() => selectGlobalPrefixFault(a, [{ ...baseline, raw: JSON.stringify(foreign), data: foreign }], mounted, latest));
  for (const url of [mounted + '&source_transcript=' + source, mounted.replace(source, 'foreign'), mounted.split('?')[0]]) {
    assert.throws(() => selectGlobalPrefixFault(a, snapshots, url, latest));
  }
  assert.throws(() => selectGlobalPrefixFault(a, snapshots.slice(0, 1), mounted, latest));
  assert.throws(() => selectGlobalPrefixFault(a, snapshots, mounted, 'unknown-latest'));
  console.log(JSON.stringify({ evidence: dir, selected, raw_sha256: createHash('sha256').update(baseline.raw).digest('hex') }));
});
test('pinned d4 rendered prefix sentinel/error and exact physical selectors remain the contract', { skip: !sourceRoot }, () => {
  const source = readFileSync(`${sourceRoot}/ui/src/components/MessageList.tsx`, 'utf8');
  assert.ok(source.includes("if (visibleRange.startIndex <= 2) requestEarlierHistory('range');"));
  assert.ok(source.includes('onLoadOlderMessages(restoreBasis);'));
  assert.ok(source.includes(GLOBAL_PREFIX_ERROR_TEXT));
  assert.ok(source.includes('olderHistoryError && hasOlderMessages && onLoadOlderMessages'));
  assert.ok(source.includes('onRangeChange={handleRangeChanged}'));
  const driver = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const trigger = driver.indexOf('await page.mouse.wheel(0, deltaY);');
  const failed = driver.indexOf("'actual failed Global prefix GET'");
  const destroyed = driver.indexOf("assert.ok(destroyed,");
  const baseline = driver.indexOf("postfaultbaseline = await declarePostFaultBaseline(prefixBudget)");
  const release = driver.indexOf('await releaseOnce(gate, command);');
  assert.ok(trigger < failed && failed < destroyed && destroyed < baseline && baseline < release);
  assert.ok(driver.includes('prefixDeadline = performance.now() + 20000'));
  assert.ok(driver.includes('globalFaultObservation?.request ?? await poll'));
  assert.doesNotMatch(driver, /page\.route\(|dispatchEvent\(|__react|evaluate.*onLoadOlder/);
});
