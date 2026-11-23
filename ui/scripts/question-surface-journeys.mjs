import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';

// Runs after measurement, against disposable HTTP fixtures only.
export async function qualifyQuestionSurface({ page, out, questions, setQuestions, setAnswerError }) {
  const evidence = [];
  const dialog = page.locator('.coordinator-question-dock__dialog');
  const custom = page.getByRole('textbox', { name: 'Write a custom answer' });
  const launcher = page.getByRole('button', { name: 'Answer Ember’s question' });
  const refresh = () => page.evaluate(() => window.dispatchEvent(new Event('focus')));
  const screenshot = name => page.screenshot({ path: path.join(out, `journey-${name}.png`) });
  const geometry = () => dialog.evaluate(el => ({
    modal: el.matches(':modal'),
    scrollables: [...el.querySelectorAll('*')].filter(child => /(auto|scroll)/.test(getComputedStyle(child).overflowY) && child.scrollHeight > child.clientHeight + 1).map(child => child.className),
    bottom: el.getBoundingClientRect().bottom, height: innerHeight,
  }));
  const retained = await custom.inputValue();
  await page.getByRole('button', { name: 'Return to conversation' }).click();
  const composer = page.locator('#message-input');
  await composer.waitFor({ state: 'visible' });
  await composer.fill('Disposable retained conversation draft');
  await launcher.click();
  assert.equal(await custom.inputValue(), retained);
  const initial = await geometry();
  assert.equal(initial.modal, true);
  assert.deepEqual(initial.scrollables, ['coordinator-question-dock__scroller']);
  assert.ok(initial.bottom <= initial.height + 1);
  // Native modality must reject focus on the background, not merely hide it.
  if (await composer.count()) {
    assert.equal(await composer.evaluate(el => { el.focus(); return document.activeElement === el; }), false);
  }
  await screenshot('mobile');
  evidence.push({ journey: 'return/reopen, modal focus exclusion, single scroller', initial });
  await custom.focus();
  await page.keyboard.press('Escape');
  assert.equal(await dialog.evaluate(el => el.open), true);
  assert.equal(await custom.evaluate(el => el === document.activeElement), false);
  await page.getByRole('button', { name: 'Dismiss', exact: true }).click();
  await page.locator('.confirm-dialog').waitFor({ state: 'visible' });
  await page.keyboard.press('Escape');
  await page.locator('.confirm-dialog').waitFor({ state: 'detached' });
  assert.equal(await dialog.evaluate(el => el.open), true);
  evidence.push({ journey: 'input Escape and confirmation Escape preserve outer modal' });
  const original = page.viewportSize();
  await page.setViewportSize({ width: original.width, height: 520 });
  await page.waitForFunction(() => document.querySelector('.coordinator-question-dock__dialog').getBoundingClientRect().bottom <= innerHeight + 1);
  const reduced = await geometry();
  assert.deepEqual(reduced.scrollables, ['coordinator-question-dock__scroller']);
  await page.locator('.question-actions').scrollIntoViewIfNeeded();
  await screenshot('reduced-viewport');
  await page.setViewportSize(original);
  evidence.push({ journey: 'reduced viewport (not OS keyboard)', reduced });
  // Feed identity changes are delivered through the real HTTP polling boundary.
  const request = { ...questions[0], request_id: 'journey-wizard', questions: [
    { header: 'First', question: 'Choose or write a custom answer', options: [{ label: 'One' }, { label: 'Two' }], multiSelect: false },
    { header: 'Second', question: 'Choose two checks', options: [{ label: 'Alpha' }, { label: 'Beta' }], multiSelect: true },
  ] };
  setQuestions([request]); await refresh();
  await page.getByText('Choose or write a custom answer', { exact: true }).waitFor();
  await custom.fill('Journey custom answer');
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  for (const label of ['Alpha', 'Beta']) await page.locator('.question-option').filter({ hasText: label }).click();
  setAnswerError(true);
  await page.getByRole('button', { name: 'Submit', exact: true }).click();
  await page.locator('.question-feedback--error').waitFor();
  setAnswerError(false);
  const answerRequest = page.waitForRequest(req => req.url().endsWith('/journey-wizard/answer') && req.method() === 'POST');
  await page.getByRole('button', { name: 'Submit', exact: true }).click();
  const payload = (await answerRequest).postDataJSON();
  assert.equal(payload.answers[request.questions[0].question], 'Journey custom answer');
  assert.ok(payload.answers[request.questions[1].question].includes('Alpha'));
  assert.ok(payload.answers[request.questions[1].question].includes('Beta'));
  await dialog.waitFor({ state: 'detached' });
  evidence.push({ journey: 'multi-step/custom/multi-select, failure then retry', payload });
  // Withdrawal while open removes the obsolete surface without submitting it.
  setQuestions([{ ...request, request_id: 'journey-withdrawn' }]); await refresh();
  await dialog.waitFor({ state: 'visible' });
  setQuestions([]); await refresh();
  await dialog.waitFor({ state: 'detached' });
  evidence.push({ journey: 'withdrawn request disappears on feed refresh' });
  if (await composer.count()) assert.equal(await composer.inputValue(), 'Disposable retained conversation draft');
  await fs.writeFile(path.join(out, 'question-surface-journeys.json'), JSON.stringify({ evidence, limitations: ['Hosted Chromium, not real iPhone/keyboard.', 'Browser history traversal is not equivalent to native dialog cancel and is not qualified.'] }, null, 2));
}
