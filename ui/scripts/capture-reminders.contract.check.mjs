import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const [stories, mockApi, capture, fixture] = await Promise.all([
  readFile(new URL('../src/stories/product-conversation.stories.tsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/fixtures/productConversation/mockApi.ts', import.meta.url), 'utf8'),
  readFile(new URL('./capture-reminders.mjs', import.meta.url), 'utf8'),
  readFile(new URL('../src/fixtures/productConversation/renderFixture.tsx', import.meta.url), 'utf8'),
]);

test('Reminders capture contract has typed scenarios, conversation-scoped GETs and bounded journeys', () => {
  for (const id of ['reminders-initial-failure', 'reminders-alias-fallback-initial-failure', 'reminders-loaded-stale-upcoming', 'reminders-retry-mutation-rejection']) {
    assert.ok(stories.includes(`withReminders('${id}'`), `Missing Reminders story scenario ${id}`);
  }
  assert.doesNotMatch(stories, /as ProductConversationScenarioId/);
  for (const suffix of ['initial-failure', 'alias-fallback-initial-failure', 'loaded-stale-upcoming', 'retry-mutation-rejection']) {
    assert.ok(capture.includes(`'${suffix}'`), `Capture must use the suffix after product-conversation--reminders-: ${suffix}`);
  }
  assert.match(capture, /readyAttribute: 'data-reminders-fixture-ready'/);
  assert.match(fixture, /'data-reminders-fixture-ready': scenario.id.slice\('reminders-'.length\)/);
  assert.match(fixture, /path="\/c\/:slug" element=\{<FixtureAlias \/>\}/);
  assert.match(fixture, /scenario.aliasFallbackNotFound/);
  assert.match(mockApi, /scenario.aliasFallbackNotFound && reference === scenario.snapshot\?\.latest_transcript_row_id/);
  assert.match(mockApi, /new ApiResponseError\('Conversation aggregate not found', 404\)/);
  assert.match(capture, /\.conversation-alias-fallback #app \.conversation-column #chat-view/);
  assert.match(mockApi, /`\/api\/conversations\/\$\{encodeURIComponent\(conversation\.id\)\}\/reminders`/);
  assert.match(mockApi, /failuresAfterFirstLoad/);
  assert.match(mockApi, /status: 409/);
  assert.match(capture, /storyPrefix: 'product-conversation--reminders-'/);
  assert.match(capture, /'mobile-375x667'.*'mobile-393x852'/s);
  assert.match(capture, /409/);
});
