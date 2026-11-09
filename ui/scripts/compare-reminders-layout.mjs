import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';

const afterDir = process.env.REMINDERS_QA_OUT ?? 'qa-artifacts/reminders';
const beforeDir = process.env.REMINDERS_BASELINE_QA_OUT ?? 'qa-artifacts/reminders-baseline';
const viewports = ['mobile-375x667', 'mobile-393x852', 'desktop'];
const comparisons = [];
for (const story of ['initial-failure', 'alias-fallback-initial-failure']) {
  for (const viewport of viewports) {
    const stem = `${story}--${viewport}--initial-failure.json`;
    const before = JSON.parse(await readFile(path.join(beforeDir, stem), 'utf8'));
    const after = JSON.parse(await readFile(path.join(afterDir, stem), 'utf8'));
    const oldPanel = before.geometry['.reminders-panel'];
    const newStatus = after.geometry['.reminders-fetch-status'];
    const transcriptSelector = story === 'alias-fallback-initial-failure' ? '.conversation-alias-fallback #app .conversation-column #chat-view' : '.product-conversation-page__transcript';
    const oldTranscript = before.geometry[transcriptSelector];
    const newTranscript = after.geometry[transcriptSelector];
    assert.ok(oldPanel && newStatus && oldTranscript && newTranscript, `${stem}: required bounds missing`);
    assert.ok(oldPanel.height - newStatus.height >= 15, `${stem}: failed fetch did not save meaningful vertical space`);
    assert.ok(newTranscript.height >= oldTranscript.height + 15, `${stem}: transcript did not regain space`);
    const composer = after.geometry['#input-area'];
    assert.ok(composer?.height > 0 && composer.y >= 0 && composer.bottom <= after.viewport.height, `${stem}: composer displaced out of viewport`);
    comparisons.push({ story, viewport, oldPanelHeight: oldPanel.height, newStatusHeight: newStatus.height,
      oldTranscriptHeight: oldTranscript.height, newTranscriptHeight: newTranscript.height,
      beforeComposer: before.geometry['#input-area'], afterComposer: after.geometry['#input-area'],
      beforeDocument: before.document, afterDocument: after.document });
  }
}
await writeFile(path.join(beforeDir, 'comparison.json'), JSON.stringify(comparisons, null, 2));
console.log('✓ baseline vs corrected reminders failure layout:', comparisons);
