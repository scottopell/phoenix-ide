import test from 'node:test';
import assert from 'node:assert/strict';
import { loopback, sourceLink, anchorMatches, occurrences, belongsToAggregate } from './qualify-history-realserver.mjs';

test('only literal HTTP loopback ports are accepted', () => {
  assert.equal(loopback('http://127.0.0.1:54321'), 'http://127.0.0.1:54321');
  for (const value of ['https://127.0.0.1:1', 'http://localhost:1', 'http://example.com:1', 'http://127.0.0.1', 'http://x:y@127.0.0.1:1', 'http://127.0.0.1:1/path']) {
    assert.throws(() => loopback(value));
  }
});
test('exact source pins and tool/message escaping retain unrelated query', () => {
  const url = new URL(sourceLink({ route: '/global/leaf' }, 'root', 'message/id', 'tool:id'), 'http://127.0.0.1:1');
  assert.equal(url.searchParams.get('source_transcript'), 'root');
  assert.equal(url.searchParams.get('source_tool'), 'tool:id');
  assert.equal(url.searchParams.get('qualification'), 'retained');
  assert.equal(url.hash, '#message-message%2Fid');
});
test('physical anchor requires same stable key and normative two-pixel agreement', () => {
  const before = { key: 'same', offset: -3 };
  assert.equal(anchorMatches(before, { key: 'same', offset: -1 }), true);
  assert.equal(anchorMatches(before, { key: 'same', offset: -.99 }), false);
  assert.equal(anchorMatches(before, { key: 'different', offset: -3 }), false);
  assert.equal(anchorMatches(before, null), false);
  assert.equal(anchorMatches(before, { key: 'same', offset: NaN }), false);
});
test('pagination occurrences distinguish sequence restarts across real members', () => {
  const snapshot = { segments: [0, 1, 2].map(n => ({ segment_ordinal: n, messages: [{ sequence_id: 1, message_id: `row${n}` }] })) };
  assert.deepEqual(occurrences(snapshot), [[0, 1, 'row0'], [1, 1, 'row1'], [2, 1, 'row2']]);
});
test('Global responses can change pinned member without changing aggregate identity', () => {
  const a = { kind: 'global', members: ['root', 'child'] };
  const response = { url: 'http://127.0.0.1:1/api/global/coordinator/history/child?before=real', data: { canonical_root: { transcript_row_id: 'root' } } };
  assert.equal(belongsToAggregate(a, response), true);
  assert.equal(belongsToAggregate({ ...a, members: ['foreign'] }, response), false);
  assert.equal(belongsToAggregate({ kind: 'ordinary', endpoint: '/api/product-conversations/pc' }, { ...response, url: 'http://127.0.0.1:1/api/product-conversations/pc?before=real' }), true);
});
