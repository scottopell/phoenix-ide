import test from 'node:test';
import assert from 'node:assert/strict';
import { runInNewContext } from 'node:vm';
import { readFileSync } from 'node:fs';
import { historicalMessageIdentities, observePhysicalHistoricalRow, holdPostFaultBaseline, physicalBaselineMatches } from './history-postfault-baseline.mjs';

// Fabricated DOM/geometry UNIT ONLY; not an old or current browser DOM snapshot.
const fixture = { kind: 'global', members: ['root', 'middle'], occurrences: [[0, 2, 'root:known'], [1, 3, 'middle:known']] };
const known = historicalMessageIdentities(fixture);
const box = (top = 110, height = 50) => ({ top, bottom: top + height, height, left: 10, right: 310, width: 300 });
function dom({ id = 'message-root:known', data = null, key = 'agent-group-unknown', childBox = box(120, 20), rowBox = box(), nested = false, hidden = false, empty = false, scrollTop = 0 } = {}) {
  const row = { dataset: { virtualKey: key }, getBoundingClientRect: () => rowBox, getClientRects: () => [rowBox] };
  const child = { getAttribute: name => name === 'id' ? id : data, getBoundingClientRect: () => childBox,
    getClientRects: () => [childBox], closest: () => nested ? {} : row };
  row.querySelectorAll = () => empty ? [] : [child];
  const scroller = { getBoundingClientRect: () => box(100, 729), querySelectorAll: () => [row], scrollTop, scrollHeight: 21806, clientHeight: 729 };
  const context = { document: { querySelector: () => scroller }, innerHeight: 900, innerWidth: 1360,
    getComputedStyle: () => ({ display: 'block', visibility: hidden ? 'hidden' : 'visible' }),
    location: { href: 'http://127.0.0.1:123/global/root?source_transcript=middle#message-middle%3Aknown' }, URL };
  return expected => runInNewContext(`(${observePhysicalHistoricalRow.toString()})`, context)({ known, expected });
}

test('serialized installed DOM probe declares exact root source, independently of unchanged middle pin and obsolete pixel restore', async () => {
  // Actual 3ec dimensional receipts motivate the test, but these boxes are explicitly fabricated.
  for (const scrollTop of [0, 7383]) {
    const read = dom({ scrollTop }); const first = read();
    assert.equal(first.persistedMessageID, 'root:known'); assert.equal(first.sourceRow, 'root');
    assert.equal(first.sourcepin, 'middle'); assert.notEqual(first.persistedMessageID, 'middle:known');
    const baseline = await holdPostFaultBaseline(first, read, async () => {});
    assert.equal(baseline.stability_frames.length, 4); assert.match(baseline.observation_scope, /not prefault/);
    assert.equal(baseline.elementID, 'message-root:known'); assert.equal(baseline.DOMbounds.message.height, 20);
  }
});

test('fixture mapping rejects unknown historical ordinal, empty ID and duplicate occurrence', () => {
  for (const occurrences of [[[2, 2, 'pending:known']], [[0, 2, '']], [[0, 2, 'root:known'], [0, 3, 'root:known']]])
    assert.throws(() => historicalMessageIdentities({ ...fixture, occurrences }));
});

for (const [name, options] of Object.entries({
  unknown_virtual_group_only: { id: null }, optimistic: { id: 'message-root:optimistic' }, new_opening: { id: 'message-pending:opening' },
  raw_ID_without_documented_wrapper: { id: 'root:known' }, double_wrapper: { id: 'message-message-root:known' }, foreign: { id: 'message-foreign:known' },
  mismatched_dual_identity: { data: 'middle:known' }, empty: { empty: true }, clipped_message: { childBox: box(99, 20) },
  clipped_row: { rowBox: box(99, 50) }, zero_message: { childBox: box(120, 0) }, zero_row: { rowBox: box(110, 0) },
  hidden: { hidden: true }, nested_other_virtual_row: { nested: true }, missing_virtual_key: { key: '' },
})) test(`installed DOM probe rejects ${name}`, () => assert.equal(dom(options)(), null));

test('exact data-message-id is accepted without stripping or reading text', () => {
  const value = dom({ id: null, data: 'root:known' })();
  assert.equal(value.persistedMessageID, 'root:known'); assert.equal(value.dataMessageID, 'root:known');
  assert.equal(dom({ id: null, data: 'message-root:known' })(), null);
});

test('same virtual group is insufficient when declared persisted child disappears or source changes', () => {
  const baseline = dom()();
  assert.equal(dom({ id: 'message-middle:known' })(baseline), null);
  assert.equal(physicalBaselineMatches(baseline, { ...baseline, sourceRow: 'middle' }), false);
  assert.equal(physicalBaselineMatches(baseline, { ...baseline, messageOffset: baseline.messageOffset + 3 }), false);
  assert.equal(physicalBaselineMatches(baseline, { ...baseline, sourcepin: 'root' }), false);
});

test('declared baseline rejects unstable frames and missing persisted child, accepts inclusive 2px', async () => {
  const first = dom()();
  for (const read of [() => null, () => ({ ...first, offset: first.offset + 2.01 }), () => ({ ...first, messageOffset: first.messageOffset + 3 })])
    await assert.rejects(holdPostFaultBaseline(first, read, async () => {}));
  await assert.rejects(holdPostFaultBaseline(null, dom(), async () => {}));
  const result = await holdPostFaultBaseline(first, () => ({ ...first, offset: first.offset + 2, messageOffset: first.messageOffset - 2 }), async () => {});
  assert.equal(result.stability_frames.length, 4);
});

test('driver declares postfault baseline before release and retains independent virtual anchor subsequence', () => {
  const source = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const start = source.indexOf("      checked('Global earlier-history acquisition failed while Pending");
  const end = source.indexOf('    await releaseOnce(gate, command);');
  const action = source.slice(start, end);
  assert.match(action, /postfaultbaseline = await declarePostFaultBaseline\(prefixBudget\)/);
  assert.match(action, /checkedGatePending: true/); assert.match(action, /gate.releaseAttempted, false/);
  assert.match(action, /const before = a.kind === 'global' \? postfaultbaseline/);
  assert.doesNotMatch(action, /savedTop|scrollIntoView|scrollTop\s*=|__react|dispatchEvent/);
  assert.match(source, /const value = await anchor\(before.key\)/);
  assert.match(source, /anchorMatches\(before, value\)/); assert.match(source, /physicalBaselineMatches\(before, physical\)/);
  assert.match(source, /await sameAnchor\(before, 'acceptance retains physical anchor'\)/);
  assert.match(source, /await sameAnchor\(before, 'reconnect retains physical anchor'\)/);
});

const actualFixture = process.env.HISTORY_GLOBAL_3EC_FIXTURE;
test('actual3ec retained fixture maps every exact persisted ID to its known historical ordinal, including opaque IDs', { skip: !actualFixture }, () => {
  const a = JSON.parse(readFileSync(actualFixture));
  const identities = historicalMessageIdentities(a);
  assert.equal(identities.length, 232); assert.equal(a.members.length, 4);
  identities.forEach((identity, i) => {
    assert.equal(identity.persistedMessageID, a.occurrences[i][2]);
    assert.equal(identity.sourceRow, a.members[a.occurrences[i][0]]);
  });
  assert.ok(identities.some(identity => !identity.persistedMessageID.includes(':')));
});
