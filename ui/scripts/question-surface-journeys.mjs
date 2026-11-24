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
  const screenshot = name => page.screenshot({ animations: 'disabled', path: path.join(out, `journey-${name}.png`) });
  const geometry = () => dialog.evaluate(el => ({
    modal: el.matches(':modal'),
    scrollables: [...el.querySelectorAll('*')].filter(child => /(auto|scroll)/.test(getComputedStyle(child).overflowY) && child.scrollHeight > child.clientHeight + 1).map(child => child.className),
    bottom: el.getBoundingClientRect().bottom, height: innerHeight,
  }));
  const transcript = page.locator('#messages');
  const confirmGeometry = async name => {
    await page.locator('.confirm-dialog').waitFor({ state: 'visible' });
    const bounds = await dialog.evaluate(el => {
      const rect = node => { const r = node.getBoundingClientRect(); return { top: r.top, left: r.left, right: r.right, bottom: r.bottom, height: r.height }; };
      const overlay = el.querySelector('.modal-overlay');
      const confirm = el.querySelector('.confirm-dialog');
      return { dialog: rect(el), overlay: rect(overlay), confirm: rect(confirm),
        covered: overlay.contains(document.elementFromPoint(innerWidth / 2, 100)),
        controls: [...confirm.querySelectorAll('button')].map(button => ({ ...rect(button), hit: button.contains(document.elementFromPoint(button.getBoundingClientRect().x + button.clientWidth / 2, button.getBoundingClientRect().y + button.clientHeight / 2)) })) };
    });
    assert.ok(Math.abs(bounds.overlay.top - bounds.dialog.top) <= 1 && Math.abs(bounds.overlay.bottom - bounds.dialog.bottom) <= 1, 'Confirmation must cover the full question viewport');
    assert.equal(bounds.covered, true, 'Question content must be covered by confirmation');
    for (const control of bounds.controls) {
      assert.ok(control.top >= bounds.dialog.top && control.bottom <= bounds.dialog.bottom && control.hit, 'Confirmation controls must be onscreen and hit-testable without scrolling');
    }
    await screenshot(name);
    evidence.push({ journey: name, bounds });
  };
  const settledScrollTop = () => transcript.evaluate(el => new Promise((resolve, reject) => {
    let previous = el.scrollTop, stable = 0, frames = 0;
    const sample = () => {
      const current = el.scrollTop;
      stable = Math.abs(current - previous) < 0.5 ? stable + 1 : 0;
      previous = current;
      if (stable >= 5) resolve(current);
      else if (++frames >= 120) reject(new Error('Transcript scroll did not settle'));
      else requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
  }));
  const retained = await custom.inputValue();
  const conversationUrl = page.url();
  const mutations = [];
  const recordMutation = req => {
    if (req.method() === 'POST' && /\/api\/coordinator\/questions\/[^/]+\/(answer|dismiss)$/.test(new URL(req.url()).pathname)) {
      mutations.push({ path: new URL(req.url()).pathname, payload: req.postDataJSON() });
    }
  };
  page.on('request', recordMutation);
  await page.getByRole('button', { name: 'Return to conversation' }).click();
  const composer = page.locator('#message-input');
  await composer.waitFor({ state: 'visible' });
  await composer.fill('Disposable retained conversation draft');
  await transcript.hover();
  await page.mouse.wheel(0, -1200);
  await page.waitForFunction(() => {
    const el = document.querySelector('#messages');
    return el && el.scrollTop > 0 && el.scrollHeight - el.clientHeight - el.scrollTop > 100;
  });
  const readingPosition = await settledScrollTop();
  await launcher.click();
  await dialog.waitFor({ state: 'visible' });
  assert.equal(await custom.inputValue(), retained);
  assert.ok(Math.abs(await settledScrollTop() - readingPosition) <= 2, 'Opening question moved transcript');
  await page.getByRole('button', { name: 'Return to conversation' }).click();
  await composer.waitFor({ state: 'visible' });
  const returnedPosition = await settledScrollTop();
  assert.ok(Math.abs(returnedPosition - readingPosition) <= 2, 'Return moved transcript');
  assert.equal(await composer.inputValue(), 'Disposable retained conversation draft');
  assert.equal(page.url(), conversationUrl);
  assert.deepEqual(mutations, [], 'Return/reopen must not answer or dismiss');
  await launcher.click();
  const initial = await geometry();
  assert.equal(initial.modal, true);
  await page.evaluate(() => window.dispatchEvent(new KeyboardEvent('keydown', { key: '?', bubbles: true })));
  assert.equal(await page.locator('.shortcut-help-panel').count(), 0, 'Global help outside native mobile modal must remain suppressed');
  assert.equal(await dialog.evaluate(el => el.matches(':modal')), true);
  assert.deepEqual(initial.scrollables, ['coordinator-question-dock__scroller']);
  assert.ok(initial.bottom <= initial.height + 1);
  // Native modality must reject focus on the background, not merely hide it.
  if (await composer.count()) {
    assert.equal(await composer.evaluate(el => { el.focus(); return document.activeElement === el; }), false);
  }
  await screenshot('mobile');
  evidence.push({ journey: 'return/reopen preserves real transcript scroll and draft; modal focus exclusion; single scroller', readingPosition, returnedPosition, initial });
  await custom.focus();
  await page.keyboard.press('Escape');
  assert.equal(await dialog.evaluate(el => el.open), true);
  assert.equal(await custom.evaluate(el => el === document.activeElement), false);
  await page.getByRole('button', { name: 'Dismiss', exact: true }).click();
  await confirmGeometry('confirmation');
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
  await page.getByRole('button', { name: 'Dismiss', exact: true }).click();
  await confirmGeometry('confirmation-reduced-viewport');
  await page.keyboard.press('Escape');
  await page.locator('.confirm-dialog').waitFor({ state: 'detached' });
  await page.setViewportSize(original);
  evidence.push({ journey: 'reduced viewport (not OS keyboard)', reduced });
  await page.locator('.question-panel').focus();
  await page.keyboard.press('?');
  await page.locator('.shortcut-help-panel').waitFor({ state: 'visible' });
  assert.equal(await dialog.evaluate(el => el.open), false);
  assert.equal(await page.locator('.shortcut-help-panel').evaluate(el => {
    const r = el.getBoundingClientRect();
    return el.contains(document.elementFromPoint(r.x + r.width / 2, r.y + 20));
  }), true);
  await page.keyboard.press('Escape');
  await page.locator('.shortcut-help-panel').waitFor({ state: 'detached' });
  await launcher.click();
  assert.equal(await custom.inputValue(), retained);
  evidence.push({ journey: 'global shortcut help releases native modality and preserves answer on reopen' });
  // Feed identity changes are delivered through the real HTTP polling boundary.
  const request = { ...questions[0], request_id: 'journey-wizard', questions: [
    { header: 'First', question: 'Choose or write a custom answer', options: [{ label: 'One' }, { label: 'Two' }], multiSelect: false },
    { header: 'Second', question: 'Choose two checks', options: [{ label: 'Alpha' }, { label: 'Beta' }], multiSelect: true },
  ] };
  const nextRequest = { ...request, request_id: 'journey-next', questions: [
    { header: 'Pending', question: 'The second pending request is ready', options: [{ label: 'Continue' }], multiSelect: false },
  ] };
  setQuestions([request, nextRequest]); await refresh();
  await page.getByText('Choose or write a custom answer', { exact: true }).waitFor();
  await page.locator('.coordinator-question-dock__more').waitFor();
  assert.equal(await page.getByText(nextRequest.questions[0].question, { exact: true }).count(), 0);
  await custom.fill('Journey custom answer');
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  for (const label of ['Alpha', 'Beta']) await page.locator('.question-option').filter({ hasText: label }).click();
  setAnswerError(true);
  await page.getByRole('button', { name: 'Submit', exact: true }).click();
  await page.locator('.question-feedback--error').waitFor();
  setAnswerError(false);
  const answerRequest = page.waitForRequest(req => req.url().endsWith('/journey-wizard/answer') && req.method() === 'POST');
  const answerResponse = page.waitForResponse(res => res.url().endsWith('/journey-wizard/answer') && res.request().method() === 'POST');
  await page.getByRole('button', { name: 'Submit', exact: true }).click();
  const payload = (await answerRequest).postDataJSON();
  assert.equal(payload.answers[request.questions[0].question], 'Journey custom answer');
  assert.ok(payload.answers[request.questions[1].question].includes('Alpha'));
  assert.ok(payload.answers[request.questions[1].question].includes('Beta'));
  assert.equal((await answerResponse).ok(), true);
  await page.getByText(nextRequest.questions[0].question, { exact: true }).waitFor();
  assert.equal(await dialog.evaluate(el => el.open), true);
  assert.equal(await page.locator('.coordinator-question-dock__more').count(), 0);
  evidence.push({ journey: 'two pending requests: multi-step/custom/multi-select, failure then retry advances to second', payload });
  const beforeDismiss = mutations.length;
  await page.getByRole('button', { name: 'Dismiss', exact: true }).click();
  const confirmation = page.locator('.confirm-dialog');
  await confirmation.waitFor({ state: 'visible' });
  assert.equal(mutations.length, beforeDismiss, 'Opening confirmation must not dismiss');
  const dismissResponse = page.waitForResponse(res => res.url().endsWith('/journey-next/dismiss') && res.request().method() === 'POST');
  await confirmation.getByRole('button', { name: 'Dismiss', exact: true }).click();
  assert.equal((await dismissResponse).ok(), true);
  await dialog.waitFor({ state: 'detached' });
  const emptyFeed = page.waitForResponse(res => new URL(res.url()).pathname === '/api/coordinator/questions' && res.request().method() === 'GET');
  await refresh();
  assert.equal((await emptyFeed).status(), 200);
  assert.equal(await dialog.count(), 0);
  assert.equal(await launcher.count(), 0);
  assert.equal(mutations.length, beforeDismiss + 1);
  assert.equal(mutations.at(-1).path, '/api/coordinator/questions/journey-next/dismiss');
  evidence.push({ journey: 'confirmed dismissal of second request empties queue', mutation: mutations.at(-1) });
  // Withdrawal while open removes the obsolete surface without submitting it.
  setQuestions([{ ...request, request_id: 'journey-withdrawn' }]); await refresh();
  await dialog.waitFor({ state: 'visible' });
  setQuestions([]); await refresh();
  await dialog.waitFor({ state: 'detached' });
  evidence.push({ journey: 'withdrawn request disappears on feed refresh' });
  if (await composer.count()) assert.equal(await composer.inputValue(), 'Disposable retained conversation draft');
  const beforeReload = mutations.length;
  const reloadRequest = { ...nextRequest, request_id: 'journey-reload' };
  setQuestions([reloadRequest]);
  const reloadedFeed = page.waitForResponse(res => new URL(res.url()).pathname === '/api/coordinator/questions' && res.request().method() === 'GET');
  await page.reload({ waitUntil: 'domcontentloaded' });
  assert.equal((await reloadedFeed).status(), 200);
  await dialog.waitFor({ state: 'visible', timeout: 45000 });
  await page.getByText(reloadRequest.questions[0].question, { exact: true }).waitFor();
  await custom.fill('Usable after reload');
  assert.equal(await custom.inputValue(), 'Usable after reload');
  assert.equal(await dialog.evaluate(el => el.matches(':modal')), true);
  await screenshot('reload-ready');
  evidence.push({ journey: 'reload reacquires pending question and permits custom input (not a latency measurement)' });

  // Navigate query variants of the canonical transcript; /global is a redirect.
  const historyStart = page.url();
  const alternatePath = `${new URL(historyStart).pathname}?question-journey=history`;
  await page.evaluate(route => window.dispatchEvent(new CustomEvent('phoenix:navigate-to-conversation', { detail: { route } })), alternatePath);
  await page.waitForURL(url => `${url.pathname}${url.search}` === alternatePath);
  await dialog.waitFor({ state: 'visible' });
  await page.goBack();
  await page.waitForURL(historyStart);
  await dialog.waitFor({ state: 'visible' });
  assert.equal(await dialog.evaluate(el => el.matches(':modal')), true);
  await page.goForward();
  await page.waitForURL(url => `${url.pathname}${url.search}` === alternatePath);
  await dialog.waitFor({ state: 'visible' });
  await custom.fill('Usable after browser Forward');
  assert.equal(await custom.inputValue(), 'Usable after browser Forward');
  assert.equal(await dialog.evaluate(el => el.matches(':modal')), true);
  assert.equal(mutations.length, beforeReload, 'Reload/history traversal must not answer or dismiss');
  await screenshot('history-forward');
  evidence.push({ journey: 'real router navigation and browser Back/Forward between canonical Ember transcript query variants', from: historyStart, to: page.url() });
  page.off('request', recordMutation);
  await fs.writeFile(path.join(out, 'question-surface-journeys.json'), JSON.stringify({ evidence, limitations: ['Hosted Chromium, not real iPhone/keyboard.', 'History qualification covers query variants of one Ember transcript only; it does not claim browser Back is native dialog cancel or that drafts survive page reload.'] }, null, 2));
}

export async function qualifyDesktopShortcuts({ page, out }) {
  const dialog = page.locator('.coordinator-question-dock__dialog');
  assert.equal(await dialog.evaluate(el => el.open && !el.matches(':modal')), true);
  const transcript = page.locator('#messages');
  await transcript.evaluate(el => { el.tabIndex = -1; el.focus(); });
  await page.keyboard.press('?');
  await page.locator('.shortcut-help-panel').waitFor({ state: 'visible' });
  await page.keyboard.press('Escape');
  await page.locator('.shortcut-help-panel').waitFor({ state: 'detached' });
  const composer = page.locator('#message-input');
  await composer.focus();
  await page.keyboard.press('Escape');
  assert.equal(await composer.evaluate(el => el === document.activeElement), false, 'Desktop composer Escape must blur outside nonmodal question');
  await dialog.getByRole('button', { name: 'Dismiss', exact: true }).click();
  await page.locator('.confirm-dialog').waitFor({ state: 'visible' });
  await page.keyboard.press('Escape');
  await page.locator('.confirm-dialog').waitFor({ state: 'detached' });
  assert.equal(await dialog.evaluate(el => el.open && !el.matches(':modal')), true, 'Nested confirmation Escape must not close desktop question');
  await page.screenshot({ animations: 'disabled', path: path.join(out, 'journey-desktop-shortcuts.png') });
  await fs.writeFile(path.join(out, 'desktop-shortcuts.json'), JSON.stringify({ outside_dock_help: true, composer_escape_blurs: true, nested_confirmation_escape: true }, null, 2));
}

export async function qualifyDesktopPlacement({ page, out, version }) {
  const dialog = page.locator('.coordinator-question-dock__dialog');
  const panel = page.locator('.coordinator-question-dock .question-panel');
  const dock = page.locator('.coordinator-question-dock');
  const frames = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  const geometry = () => page.evaluate(() => {
    const rect = el => { const r = el.getBoundingClientRect(); return { x:r.x,y:r.y,width:r.width,height:r.height,bottom:r.bottom }; };
    const dock = document.querySelector('.coordinator-question-dock');
    const dialog = dock.querySelector('dialog');
    const meta = dock.querySelector('.coordinator-question-dock__meta');
    const messages = document.querySelector('#messages');
    return { dock:rect(dock), dialog:rect(dialog), meta:rect(meta), dockScrollTop:dock.scrollTop, windowScrollY:scrollY, transcriptScrollTop:messages?.scrollTop, modal:dialog.matches(':modal'), activeClass:document.activeElement?.className };
  });
  await page.waitForFunction(() => document.activeElement?.matches('.coordinator-question-dock .question-panel'));
  await frames();
  const initial = await geometry();
  await fs.writeFile(`${out}/${version}-desktop-placement-initial.json`,JSON.stringify(initial,null,2));
  await page.screenshot({path:`${out}/${version}-desktop-placement-initial.png`});
  const headerInitiallyClipped = initial.meta.y < initial.dock.y - 1;
  if (version === 'candidate') {
    assert.equal(headerInitiallyClipped,false);
    assert.ok(initial.dialog.y >= -1);
    assert.equal(initial.dockScrollTop,0);
  }
  // Intended dock scrolling must expose both the metadata and the final controls.
  await dock.evaluate(el => {el.scrollTop=0;});
  assert.ok(await page.locator('.coordinator-question-dock__meta').evaluate(el=>el.getBoundingClientRect().y>=0));
  const first = panel.locator('.question-option').first();
  await first.click();
  const custom = panel.getByRole('textbox',{name:'Write a custom answer'});
  await custom.fill('placement preserved answer');
  const composer = page.locator('#message-input');
  await composer.fill('placement preserved composer draft');
  await page.setViewportSize({width:393,height:852});
  await frames();
  await page.locator('#messages').evaluate(el => {el.scrollTop=10000;});
  const settledTranscriptScroll = () => page.locator('#messages').evaluate(el => new Promise((resolve,reject)=>{let last=el.scrollTop,stable=0,count=0;const tick=()=>{const current=el.scrollTop;stable=Math.abs(current-last)<.5?stable+1:0;last=current;if(stable>=5)resolve(current);else if(++count>120)reject(new Error('Transcript scroll not settled'));else requestAnimationFrame(tick);};requestAnimationFrame(tick);}));
  const transcriptScroll = await settledTranscriptScroll();
  await page.getByRole('button',{name:'Return to conversation'}).click();
  const launcher=page.getByRole('button',{name:/Answer Ember/});
  await page.waitForFunction(() => document.activeElement?.matches('.coordinator-question-dock__launcher'));
  await launcher.click();
  await page.waitForFunction(() => document.activeElement?.matches('.coordinator-question-dock .question-panel'));
  await frames();
  const reopened=await geometry();
  if(version==='candidate') assert.ok(reopened.dialog.y>=0);
  assert.equal(await custom.inputValue(),'placement preserved answer');
  assert.equal(await composer.inputValue(),'placement preserved composer draft');
  assert.equal(await settledTranscriptScroll(),transcriptScroll);
  const transitions=[];
  for(const viewport of [{width:393,height:852},{width:1280,height:720}]) {
    await page.setViewportSize(viewport);
    await frames();
    assert.equal(await dialog.evaluate(el=>el.open),true);
    await page.waitForFunction(() => document.activeElement?.matches('.coordinator-question-dock .question-panel'));
    const g=await geometry();transitions.push({viewport,...g});
    assert.equal(g.modal,viewport.width===393);
    assert.equal(await custom.inputValue(),'placement preserved answer');
    if(version==='candidate') {
      assert.ok(g.meta.y>=g.dock.y-1);
      assert.ok(g.dialog.y>=-1);
    }
  }
  await dock.evaluate(el=>{el.scrollTop=el.scrollHeight;});
  const actions=panel.locator('.question-actions');
  assert.ok(await actions.evaluate(el=>{const r=el.getBoundingClientRect(),d=el.closest('.coordinator-question-dock').getBoundingClientRect();return r.y>=Math.max(0,d.y)-1&&r.bottom<=Math.min(innerHeight,d.bottom)+1;}));
  await page.screenshot({path:`${out}/${version}-desktop-placement-actions.png`});
  await dock.evaluate(el=>{el.scrollTop=0;});
  await page.screenshot({path:`${out}/${version}-desktop-placement-resized.png`});
  await composer.fill('');
  await page.setViewportSize({width:1280,height:900});
  await frames();
  await fs.writeFile(`${out}/${version}-desktop-placement.json`,JSON.stringify({initial,headerInitiallyClipped,reopened,transitions,transcriptScroll,answersAndDraftPreserved:true,launcherFocusRestored:true},null,2));
}
