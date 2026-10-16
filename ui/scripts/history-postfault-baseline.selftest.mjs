import test from 'node:test';
import assert from 'node:assert/strict';
import { runInNewContext } from 'node:vm';
import { readFileSync } from 'node:fs';
import { historicalMessageIdentities, observePhysicalHistoricalRow, holdPostFaultBaseline, physicalBaselineMatches } from './history-postfault-baseline.mjs';

// Fabricated DOM/geometry UNIT ONLY; not an old or current browser DOM snapshot.
const fixture = { kind: 'global', members: ['root', 'middle'], occurrences: [[0, 2, 'root:known'], [1, 3, 'middle:known']] };
const known = historicalMessageIdentities(fixture);
const box = (top = 110, height = 50) => ({ top, bottom: top + height, height, left: 10, right: 310, width: 300 });
function dom(options = {}) {
  const { scrollTop = 0, extraRows = [], outsideChildren = [] } = options;
  const makeRow = ({ id = 'message-root:known', data = null, key = 'agent-group-unknown', childBox = box(120, 20), rowBox = box(), nested = false, hidden = false, empty = false, children = null } = {}) => {
    const row = { dataset: { virtualKey: key }, hidden, getBoundingClientRect: () => rowBox, getClientRects: () => [rowBox] };
    const makeChild = ({ id: childID = id, data: childData = data, childBox: b = childBox, hidden: childHidden = hidden } = {}) => ({
      getAttribute: name => name === 'id' ? childID : childData, getBoundingClientRect: () => b,
      hidden: childHidden, getClientRects: () => [b], closest: () => nested ? {} : row,
    });
    row.children = empty ? [] : children ? children.map(makeChild) : [makeChild()];
    row.querySelectorAll = () => row.children;
    return row;
  };
  const rows = [makeRow(options), ...extraRows.map(makeRow)];
  const allChildren = [...rows.flatMap(row => row.children), ...outsideChildren.map(({ id, data = null }) => ({ getAttribute: name => name === 'id' ? id : data }))];
  const scroller = { getBoundingClientRect: () => box(100, 729), querySelectorAll: () => rows, scrollTop, scrollHeight: 21806, clientHeight: 729 };
  const context = { document: { querySelector: () => scroller, querySelectorAll: selector => selector === '[data-virtual-key]' ? rows : allChildren }, innerHeight: 900, innerWidth: 1360,
    getComputedStyle: el => ({ display: 'block', visibility: el.hidden ? 'hidden' : 'visible' }),
    location: { href: 'http://127.0.0.1:123/global/root?source_transcript=middle#message-middle%3Aknown' }, URL };
  return (expected, diagnostics = false) => runInNewContext(`(${observePhysicalHistoricalRow.toString()})`, context)({ known, expected, diagnostics });
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
  assert.match(source, /const value = before.persistedMessageID \? physical : await anchor\(before.key\)/);
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

const duplicateCases = {
  children_same_wrapped_ID: { children: [{}, {}] },
  children_same_data_ID: { children: [{}, { id: null, data: 'root:known' }] },
  matching_rows_same_key: { extraRows: [{}] },
  empty_row_same_key: { extraRows: [{ empty: true }] },
  same_ID_other_row: { extraRows: [{ key: 'other-row' }] },
  hidden_recycled_row_same_key: { extraRows: [{ id: 'message-foreign:known', hidden: true }] },
  zero_height_recycled_child_same_ID: { children: [{}, { childBox: box(120, 0) }] },
  conflicting_alias_elsewhere: { outsideChildren: [{ id: 'message-middle:known', data: 'root:known' }] },
};
for (const [name, options] of Object.entries(duplicateCases)) {
  test(`initial physical selection rejects duplicate ${name}`, () => assert.equal(dom(options)(), null));
  test(`expected physical selection rejects duplicate ${name}`, () => assert.equal(dom(options)(dom()()), null));
}

test('many distinct visible known candidates deterministically choose first in DOM order', () => {
  const first = dom({ extraRows: [{ id: 'message-middle:known', key: 'second' }] })();
  assert.equal(first.persistedMessageID, 'root:known');
  assert.equal(first.key, 'agent-group-unknown');
  const reversed = dom({ id: 'message-middle:known', key: 'first', extraRows: [{ key: 'second' }] })();
  assert.equal(reversed.persistedMessageID, 'middle:known');
});

test('one element with both exact identity attributes counts once, initially and in followup', () => {
  const read = dom({ data: 'root:known' });
  const first = read();
  assert.equal(first.persistedMessageID, 'root:known');
  assert.ok(physicalBaselineMatches(first, read(first)));
});

test('first eligible unambiguous candidate skips duplicated identity but followup cannot substitute', () => {
  const read = dom({ children: [{}, {}], extraRows: [{ key: 'second', id: 'message-middle:known' }] });
  assert.equal(read().persistedMessageID, 'middle:known');
  assert.equal(read(dom()()), null);
});

test('zero-height duplicate row key rejects despite distinct identity', () => {
  const read = dom({ extraRows: [{ id: 'message-middle:known', rowBox: box(110, 0) }] });
  assert.equal(read(), null); assert.equal(read(dom()()), null);
});

for (const physicalCase of [true, false]) test(`sameAnchor observes ${physicalCase ? 'Global physical' : 'ordinary generic'} anchor without mixed evaluations`, async () => {
  const source = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const start = source.indexOf('  async function sameAnchor(before, label) {');
  const end = source.indexOf('  const composerReadonly', start);
  const before = physicalCase ? dom()() : { key: 'ordinary-key', offset: 10 };
  let physicalReads = 0, genericReads = 0, frames = 0, receipt;
  const sameAnchor = runInNewContext(`(${source.slice(start, end).trim()})`, {
    page: { evaluate: async () => { frames++; }, locator: () => ({ filter: () => ({ waitFor: async () => {} }) }) },
    physicalRow: async expected => { physicalReads++; assert.equal(expected, before); return dom()(expected); },
    anchor: async key => { genericReads++; assert.equal(key, before.key); assert.equal(physicalCase, false, 'generic recycled row must never mix with physical child'); return before; },
    anchorMatches: (b, a) => !!a && b.key === a.key && Math.abs(b.offset - a.offset) <= 2,
    physicalBaselineMatches, GLOBAL_PREFIX_ERROR_TEXT: 'error',
    poll: async fn => { for (let i = 0; i < 4; i++) { const result = await fn(); if (result) return result; } assert.fail('four stable observations required'); },
    checked: (_, value) => { receipt = value; },
  });
  await sameAnchor(before, 'unit atomic observer');
  assert.equal(frames, 4); assert.equal(physicalReads, physicalCase ? 4 : 0); assert.equal(genericReads, physicalCase ? 0 : 4);
  assert.equal(receipt.after.key, before.key);
  if (physicalCase) assert.ok(physicalBaselineMatches(before, receipt.after));
});


test('UNIT diagnostic oracle preserves strict null results and exposes only bounded predicate counts', () => {
  for (const [options, predicate] of [[{rowBox:box(99,50)},'row_clipped'],[{childBox:box(99,20)},'message_clipped'],
      [{hidden:true},'row_hidden'],[{rowBox:box(110,0)},'row_nonpositive_geometry'],
      [{data:'middle:known'},'conflicting_identity'],[{children:[{},{}]},'duplicate_physical_identity']]) {
    const read=dom(options);assert.equal(read(),null);
    const probe=read(null,true);assert.equal(probe.observation,null);assert.ok(probe.predicates[predicate]>0);
    assert.ok(Object.values(probe.predicates).every(n=>Number.isInteger(n)&&n>0&&n<=512));
    assert.ok(!JSON.stringify(probe.predicates).includes('root:known'));
  }
  const read=dom();assert.deepEqual(read(null,true).observation,read());
});

test('UNIT visible exact persisted target inside oversized row still fails strict whole-row oracle', () => {
  const read=dom({rowBox:box(110,1000),childBox:box(120,20)});
  assert.equal(read(),null);
  assert.equal(read(null,true).predicates.row_clipped,1);
  assert.equal(read(null,true).predicates.known_target_row_taller_than_clip,1);
  assert.equal(read(null,true).predicates.known_target_message_fully_visible,1);
  const oversizedUser=dom({rowBox:box(110,1000),childBox:box(110,1000)});
  assert.equal(oversizedUser(),null);
  assert.equal(oversizedUser(null,true).predicates.row_clipped,1);
  assert.equal(oversizedUser(null,true).predicates.known_target_message_clipped,1);
});
