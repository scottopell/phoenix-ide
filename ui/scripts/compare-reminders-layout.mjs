import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';

const afterDir = process.env.REMINDERS_QA_OUT ?? 'qa-artifacts/reminders';
const beforeDir = process.env.REMINDERS_BASELINE_QA_OUT ?? 'qa-artifacts/reminders-baseline';
const viewports = ['mobile-375x667', 'mobile-393x852', 'desktop'];
const comparisons = [];
for (const viewport of viewports) {
  const stem = `initial-failure--${viewport}--initial-failure.json`;
  const before = JSON.parse(await readFile(path.join(beforeDir, stem), 'utf8'));
  const after = JSON.parse(await readFile(path.join(afterDir, stem), 'utf8'));
  const oldPanel = before.geometry['.reminders-panel'];
  const newStatus = after.geometry['.reminders-fetch-status'];
  const oldTranscript = before.geometry['.product-conversation-page__transcript'];
  const newTranscript = after.geometry['.product-conversation-page__transcript'];
  assert.ok(oldPanel && newStatus && oldTranscript && newTranscript, `${viewport}: required bounds missing`);
  assert.ok(oldPanel.height - newStatus.height >= 15, `${viewport}: failed fetch did not save meaningful vertical space`);
  assert.ok(newTranscript.height >= oldTranscript.height + 15, `${viewport}: transcript did not regain space`);
  assert.ok(after.geometry['#input-area']?.height > 0, `${viewport}: composer absent`);
  comparisons.push({ viewport, oldPanelHeight: oldPanel.height, newStatusHeight: newStatus.height,
    oldTranscriptHeight: oldTranscript.height, newTranscriptHeight: newTranscript.height,
    beforeComposer: before.geometry['#input-area'], afterComposer: after.geometry['#input-area'],
    beforeDocument: before.document, afterDocument: after.document });
}
await writeFile(path.join(beforeDir, 'comparison.json'), JSON.stringify(comparisons, null, 2));
console.log('✓ baseline vs corrected reminders failure layout:', comparisons);
