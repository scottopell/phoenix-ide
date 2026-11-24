import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';

// Bounded render/effect observation, not a claim that no future timer can fire.
const settle = page => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
const dispatch = (page, name, detail) => page.evaluate(({ name, detail }) => window.dispatchEvent(new CustomEvent(name, { detail })), { name, detail });

async function observe(page, selector) {
  await page.evaluate(selector => {
    const target = document.querySelector(selector);
    if (!target) throw new Error(`Missing focus observation target: ${selector}`);
    const ancestors = [];
    for (let node = target.parentElement; node; node = node.parentElement) ancestors.push(node);
    window.__focusObservation = { active: document.activeElement, target, ancestors,
      positions: ancestors.map(node => [node.scrollTop, node.scrollLeft]), calls: [] };
    if (!window.__focusNative) {
      window.__focusNative = HTMLElement.prototype.focus;
      HTMLElement.prototype.focus = function (...args) {
        window.__focusObservation?.calls.push({ id: this.id, className: this.className });
        return window.__focusNative.apply(this, args);
      };
    }
  }, selector);
}
async function unchanged(page, label, sameTarget = true, selector = '.question-panel') {
  await settle(page);
  const evidence = await page.evaluate(selector => {
    const before = window.__focusObservation;
    return { sameFocus: document.activeElement === before.active,
      sameTarget: document.querySelector(selector) === before.target,
      oldConnected: before.target.isConnected, calls: before.calls,
      before: before.positions, after: before.ancestors.map(node => [node.scrollTop, node.scrollLeft]) };
  }, selector);
  assert.equal(evidence.sameFocus, true, `${label}: stole focus`);
  assert.deepEqual(evidence.calls, [], `${label}: unsolicited focus() call`);
  assert.deepEqual(evidence.after, evidence.before, `${label}: ancestor scroll moved`);
  assert.equal(evidence.sameTarget, sameTarget, `${label}: unexpected DOM identity`);
  assert.equal(evidence.oldConnected, sameTarget, `${label}: expected remount did not detach old target`);
  return { label, ...evidence };
}
async function focused(page, selector) {
  await page.waitForFunction(selector => document.activeElement === document.querySelector(selector), selector);
}
export async function captureQuestionFocus({ page, outDir }) {
  const evidence = [];
  await focused(page, '.question-panel');
  await page.keyboard.press('Enter');
  assert.equal(await page.locator('input[name="Q1?"]').first().isChecked(), true, 'mount keyboard control');
  for (const kind of ['state_change', 'reconnect']) {
    await page.locator('#focus-outside').click();
    await page.locator('#focus-ancestor').evaluate(node => { node.scrollTop = 0; });
    await observe(page, '.question-panel');
    await dispatch(page, 'question-focus-refresh', kind);
    await page.locator(`[data-question-refresh="${kind}"]`).waitFor();
    evidence.push(await unchanged(page, kind));
  }
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  await focused(page, '.question-panel');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  assert.equal(await page.locator('input[name="Q2?"]').nth(1).isChecked(), true, 'step keyboard control');
  await page.getByRole('button', { name: 'Dismiss', exact: true }).click();
  await focused(page, '.confirm-dialog .btn-secondary');
  for (const kind of ['state_change', 'reconnect']) {
    await observe(page, '.question-panel');
    await dispatch(page, 'question-focus-refresh', kind);
    await page.locator(`[data-question-refresh="${kind}"]`).waitFor();
    evidence.push(await unchanged(page, `confirmation-${kind}`));
  }
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await focused(page, '.question-btn--dismiss-small');
  await page.locator('#focus-outside').click();
  await page.locator('#focus-ancestor').evaluate(node => { node.scrollTop = 0; });
  await dispatch(page, 'question-focus-show', false);
  await page.locator('.question-panel').waitFor({ state: 'detached' });
  await dispatch(page, 'question-focus-show', true);
  await focused(page, '.question-panel');
  await writeFile(`${outDir}/question-focus.json`, JSON.stringify({ evidence,
    controls: ['mount keyboard', 'step keyboard', 'confirmation close restores opener', 'remount focuses'],
    limitation: 'Production QuestionPanel and ConfirmDialog plus real reducer; synthetic overflow ancestor, not exact production layout or network reconnect transport.' }, null, 2));
}

export async function captureComposerFocus({ page, outDir, viewport }) {
  const evidence = [];
  const input = page.locator('#message-input');
  const phase = async value => {
    const before = await page.locator('[data-fixture-accepted-phase]').evaluate(node => Number(node.dataset.fixturePhaseSequence));
    await dispatch(page, 'product-conversation-fixture-phase', value);
    const emitted = await page.evaluate(() => ({
      sequence: Number(document.documentElement.dataset.productConversationFixtureEmittedPhaseSequence),
      member: document.documentElement.dataset.productConversationFixtureEmittedPhaseMember,
    }));
    assert.ok(emitted.sequence > before, `${value.type}: state_change was not emitted after the accepted sequence`);
    await page.waitForFunction(({ sequence, member, type }) => {
      const marker = document.querySelector('[data-fixture-accepted-phase]');
      return marker?.dataset.fixturePhaseSequence === String(sequence)
        && marker.dataset.fixturePhaseMember === member
        && marker.dataset.fixtureAcceptedPhase === type;
    }, { ...emitted, type: value.type });
    return { before, accepted: emitted.sequence, member: emitted.member, phase: value.type };
  };
  const insert = () => dispatch(page, 'phoenix:insert-draft', { text: 'Explicit draft insertion' });
  const error = { type: 'error', message: 'Background resumable focus fixture', error_kind: 'invalid_request' };
  for (const mode of ['token0-error', 'consumed-same', 'consumed-error', 'absent-insertion']) {
    if (evidence.length) { await page.reload(); }
    await input.waitFor();
    await settle(page);
    await page.waitForFunction(() => {
      const marker = document.querySelector('[data-fixture-accepted-phase]');
      const fixture = document.documentElement.dataset;
      return Boolean(marker?.dataset.fixtureStreamIncarnation)
        && marker.dataset.fixturePhaseMember === fixture.productConversationFixtureLastDeliveredInitMember
        && Number(fixture.productConversationFixtureActiveTranscriptSubscriptions) > 0;
    });
    if (mode.startsWith('consumed')) {
      await insert();
      await focused(page, '#message-input');
      assert.match(await input.inputValue(), /Explicit draft insertion/);
    }
    if (mode === 'absent-insertion') {
      await phase({ type: 'awaiting_user_response', request_id: 'focus-pending', questions: [{ question: 'Pause?', header: 'Choice', options: [{ label: 'One', description: 'First' }], multiSelect: false }] });
      await input.waitFor({ state: 'detached' });
    }
    await page.locator('#focus-outside').click();
    if (mode === 'absent-insertion') {
      await insert();
      await phase(error);
      await input.waitFor();
      await focused(page, '#message-input');
      assert.match(await input.inputValue(), /Explicit draft insertion/);
      evidence.push({ mode, focused: true, inserted: true });
    } else {
      await observe(page, '#message-input');
      const stateChange = await phase(mode === 'consumed-same' ? { type: 'idle' } : error);
      if (mode !== 'consumed-same') await page.locator('.error-input-area').waitFor();
      evidence.push({ ...await unchanged(page, mode, mode === 'consumed-same', '#message-input'), stateChange });
      if (mode.startsWith('consumed')) assert.match(await input.inputValue(), /Explicit draft insertion/);
    }
  }
  await writeFile(`${outDir}/composer-focus-${viewport.name}.json`, JSON.stringify({ evidence,
    limitation: 'Full ProductConversationFixture ordinary-lazy-history with fixture EventSource state_change and production draft insertion. Fixture data and fixed outside focus target, not production-network or physical-device acceptance.' }, null, 2));
}
