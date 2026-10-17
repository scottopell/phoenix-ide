import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ViewJourneyLifecycle, defaultViewIdentity } from './conversationViewLifecycle';
import { ViewJourneyReporter } from './conversationViewTelemetry';

const productId = crypto.randomUUID();
const segmentId = crypto.randomUUID();
let now = 0;
let controllers: ViewJourneyLifecycle[] = [];
const send = vi.fn<typeof fetch>();
function view(route: 'global' | 'ordinary' = 'ordinary') {
  const reporter = new ViewJourneyReporter({ sampleRate: 1, send });
  const controller = new ViewJourneyLifecycle(defaultViewIdentity(route, 'navigation'), reporter, () => now);
  controllers.push(controller);
  const root = document.createElement('main');
  root.innerHTML = '<section><div id="messages"></div><div id="conversation-nav-trail"><span class="conversation-nav-item" role="button" aria-disabled="true">Private breadcrumb</span></div></section>';
  document.body.append(root);
  controller.attach(root);
  return { root, controller };
}
function hydrate(root: HTMLElement, controller: ViewJourneyLifecycle) {
  root.querySelector('#messages')!.innerHTML = '<div data-render-unit-key="private-key"><article>Private transcript</article></div>';
  controller.snapshotReceived(productId, segmentId);
  controller.hydrationReady(productId, segmentId, 1499);
  controller.chaptersReady(12);
}
function enable(root: HTMLElement) { root.querySelector('[role="button"]')!.removeAttribute('aria-disabled'); }
function payload(index = 0) { return JSON.parse(send.mock.calls[index]![1]!.body as string); }

describe('ViewJourneyLifecycle', () => {
  beforeEach(() => {
    now = 0;
    controllers = [];
    vi.useFakeTimers();
    send.mockReset().mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal('requestAnimationFrame', vi.fn(() => 1));
    vi.stubGlobal('cancelAnimationFrame', vi.fn());
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({ x: 0, y: 0, top: 0, left: 0, bottom: 100, right: 100, width: 100, height: 100, toJSON: () => ({}) });
  });
  afterEach(async () => {
    controllers.forEach(controller => controller.finish('canceled'));
    await vi.advanceTimersByTimeAsync(0);
    document.body.innerHTML = '';
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it('separates a delayed network snapshot, meaningful content, and later enabled breadcrumb', async () => {
    const { root, controller } = view();
    controller.requestStarted(crypto.randomUUID());
    now = 1000;
    hydrate(root, controller);
    now = 1005;
    controller.inspect();
    now = 1100;
    enable(root);
    controller.tailAnchorSettled();
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(payload()).toMatchObject({
      outcome: 'ready', product_conversation_id: productId, segment_id: segmentId,
      message_count_bucket: '1001+', chapter_count_bucket: '11-100', cache: 'network',
      anchor_result: 'tail_pinned', breadcrumb_ready_minus_meaningful_ms: 95,
      marks: { snapshot_received: 1000, meaningful_transcript: 1005, breadcrumb_enabled: 1100 },
      presentation_evidence: 'commit_frame_estimate',
    });
    expect(send.mock.calls[0]![1]!.body).not.toMatch(/Private|private-key/);
  });

  it('does not count retained Global content or command inventory while validation hides the ancestor', async () => {
    const { root, controller } = view('global');
    root.querySelector('section')!.setAttribute('hidden', '');
    hydrate(root, controller);
    enable(root);
    controller.tailAnchorSettled();
    controller.inspect();
    controller.finish('incomplete');
    await vi.advanceTimersByTimeAsync(0);
    expect(payload()).toMatchObject({ outcome: 'incomplete', marks: { meaningful_transcript: null, breadcrumb_presented: null, breadcrumb_enabled: null, anchor_settled: null } });
  });

  it('waits for validation release and never treats snapshot or skeleton availability as meaningful content', async () => {
    const { root, controller } = view('global');
    controller.hydrationReady(productId, segmentId, 10);
    root.querySelector('#messages')!.innerHTML = '<div class="message-skeleton"></div><div data-render-unit-key="empty-slot"></div>';
    root.querySelector('section')!.setAttribute('hidden', '');
    enable(root);
    controller.inspect();
    expect(send).not.toHaveBeenCalled();
    now = 30;
    root.querySelector('section')!.removeAttribute('hidden');
    controller.inspect();
    expect(send).not.toHaveBeenCalled();
    now = 90;
    hydrate(root, controller);
    controller.tailAnchorSettled();
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(payload()).toMatchObject({ outcome: 'ready', breadcrumb_ready_minus_meaningful_ms: -60, marks: { meaningful_transcript: 90, breadcrumb_presented: 30 } });
  });

  it.each(['target_missing', 'superseded'] as const)('records %s without fabricating target settlement', async result => {
    const { root, controller } = view();
    controller.expectTarget(true);
    hydrate(root, controller);
    enable(root);
    controller.tailAnchorSettled();
    controller.inspect();
    expect(send).not.toHaveBeenCalled();
    controller.anchor(result);
    await vi.advanceTimersByTimeAsync(0);
    expect(payload()).toMatchObject({ outcome: 'incomplete', anchor_result: result, marks: { anchor_settled: null } });
  });

  it('requires the owning applied result for a target even if the default tail is pinned', async () => {
    const { root, controller } = view();
    controller.expectTarget(true, 'target');
    hydrate(root, controller);
    root.querySelector('article')!.id = 'message-target';
    root.querySelector('article')!.innerHTML = '<span>Private target</span>';
    enable(root);
    controller.tailAnchorSettled();
    controller.inspect();
    expect(send).not.toHaveBeenCalled();
    now = 70;
    controller.anchor('applied');
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(payload()).toMatchObject({ outcome: 'ready', anchor_result: 'applied', marks: { anchor_settled: 70 } });
  });

  it('does not mark an applied target while the transcript remains blank or the target has no rendered content', async () => {
    const { root, controller } = view();
    controller.expectTarget(true, 'target');
    controller.hydrationReady(productId, segmentId, 10);
    enable(root);
    controller.anchor('applied');
    controller.inspect();
    root.querySelector('#messages')!.innerHTML = '<div data-render-unit-key="other"><article>Other visible message</article></div><div data-render-unit-key="target-slot"><div id="message-target"></div></div>';
    controller.inspect();
    controller.finish('incomplete');
    await vi.advanceTimersByTimeAsync(0);
    expect(payload()).toMatchObject({ outcome: 'incomplete', anchor_result: null, marks: { anchor_settled: null } });
  });

  it('marks applied settlement only after the actual target is visibly rendered', async () => {
    const { root, controller } = view();
    controller.expectTarget(true, 'target');
    hydrate(root, controller);
    enable(root);
    controller.anchor('applied');
    controller.inspect();
    expect(send).not.toHaveBeenCalled();
    now = 90;
    root.querySelector('article')!.id = 'message-target';
    root.querySelector('article')!.innerHTML = '<span>Private target</span>';
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(payload()).toMatchObject({ outcome: 'ready', anchor_result: 'applied', marks: { anchor_settled: 90 } });
  });

  it('assigns fresh resume identity, preserves actual retained identity, and does not reuse old anchor evidence', async () => {
    const { root, controller } = view('global');
    hydrate(root, controller);
    enable(root);
    controller.tailAnchorSettled();
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    const first = payload();
    now = 200;
    controller.resume();
    controller.inspect();
    expect(send).toHaveBeenCalledTimes(1);
    controller.tailAnchorSettled();
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(payload(1)).toMatchObject({ trigger: 'pwa_resume', cache: 'retained', product_conversation_id: productId, segment_id: segmentId, marks: { snapshot_received: null, hydration_ready: 0 } });
    expect(payload(1).open_id).not.toBe(first.open_id);
    expect(payload(1).reporter_id).toBe(first.reporter_id);
  });

  it.each(['ordinary', 'global'] as const)('keeps captured additional requests in the original ready %s report', async route => {
    const {root, controller} = view(route);
    const originalId = controller.openId;
    const owner = controller.captureRequestOwner();
    const ids = [crypto.randomUUID(), crypto.randomUUID(), crypto.randomUUID()];
    const observations = ids.map(id => owner(id)!);
    observations.forEach((observation, index) => observation.response({size_bucket: '0-10k', trace_id: String(index + 1).repeat(32)}));
    hydrate(root, controller);
    enable(root);
    controller.tailAnchorSettled();
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(payload()).toMatchObject({open_id: originalId, route, outcome: 'ready'});
    expect(payload().correlations.filter((c: {kind: string}) => c.kind === 'request').map((c: {id: string}) => c.id)).toEqual(ids);
    expect(owner(crypto.randomUUID())).toBeUndefined();
    expect(controller.captureRequestOwner()(crypto.randomUUID())).toBeUndefined();
    const frozen = JSON.stringify(payload());
    observations.forEach(observation => {observation.response({size_bucket: '1m+', trace_id: 'f'.repeat(32)}); observation.failed();});
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    expect(JSON.stringify(payload())).toBe(frozen);
  });

  it('does not let an old pagination owner register recovery requests or responses in a resumed attempt', async () => {
    const {root, controller} = view();
    const owner = controller.captureRequestOwner();
    const oldId = crypto.randomUUID();
    const stale = owner(oldId)!;
    hydrate(root, controller);
    controller.resume();
    await vi.advanceTimersByTimeAsync(0);
    expect(payload().correlations).toContainEqual({kind: 'request', id: oldId});
    expect(owner(crypto.randomUUID())).toBeUndefined();
    stale.response({size_bucket: '1m+', trace_id: 'f'.repeat(32)});
    stale.failed();
    enable(root);
    controller.tailAnchorSettled();
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(payload(1)).toMatchObject({outcome: 'ready', correlations: [], response_size_bucket: 'unknown'});
  });

  it('does not attach a pre-resume request completion or failure to a fresh resume journey', async () => {
    const { root, controller } = view();
    const oldRequest = controller.requestStarted(crypto.randomUUID())!;
    hydrate(root, controller);
    controller.resume();
    await vi.advanceTimersByTimeAsync(0);
    oldRequest.response({ size_bucket: '1m+', trace_id: '1234567890abcdef1234567890abcdef' });
    oldRequest.snapshot(crypto.randomUUID(), crypto.randomUUID());
    oldRequest.failed();
    enable(root);
    controller.tailAnchorSettled();
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(payload(1)).toMatchObject({ outcome: 'ready', product_conversation_id: productId, segment_id: segmentId, response_size_bucket: 'unknown', correlations: [], marks: { snapshot_received: null } });
  });

  it('rejects off-viewport content even when the DOM and style are mounted', async () => {
    const { root, controller } = view();
    hydrate(root, controller);
    enable(root);
    const content = root.querySelector('article')!;
    vi.spyOn(content, 'getBoundingClientRect').mockReturnValue({ x: 0, y: 10_000, top: 10_000, left: 0, bottom: 10_100, right: 100, width: 100, height: 100, toJSON: () => ({}) });
    controller.tailAnchorSettled();
    controller.inspect();
    controller.finish('incomplete');
    await vi.advanceTimersByTimeAsync(0);
    expect(payload()).toMatchObject({ marks: { meaningful_transcript: null, anchor_settled: null } });
  });

  it('reports scan-budget omissions rather than silently calling them absent', async () => {
    const { root, controller } = view();
    hydrate(root, controller);
    root.querySelector('#messages')!.innerHTML = Array.from({ length: 65 }, (_, index) => `<div data-render-unit-key="${index}"><article ${index < 64 ? 'hidden' : ''}>Private</article></div>`).join('');
    controller.inspect();
    controller.finish('incomplete');
    await vi.advanceTimersByTimeAsync(0);
    expect(payload()).toMatchObject({ observer_scan_limited: true, observer_inspection_count: 1, marks: { meaningful_transcript: null } });
  });

  it.each(['pwa_resume', 'back_forward'] as const)('observes fresh retained tail on %s without a new scroll event', async trigger => {
    const { root, controller } = view();
    hydrate(root, controller);
    enable(root);
    controller.tailAnchorSettled();
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    const probe = vi.fn(() => ({ kind: 'tail' as const, settled: true }));
    controller.registerAnchorProbe(probe);
    now = 90;
    controller.resume(trigger);
    expect(controller.openId).toBeDefined();
    await Promise.resolve();
    now = 95;
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(probe).toHaveBeenCalled();
    expect(payload(1)).toMatchObject({ outcome: 'ready', trigger, marks: { anchor_settled: 5 } });
  });

  it('rejects supersession and hidden retained targets before a fresh applied observation', async () => {
    const { root, controller } = view();
    controller.expectTarget(true, 'target');
    hydrate(root, controller);
    enable(root);
    const target = root.querySelector('article')!;
    target.id = 'message-target';
    target.innerHTML = '<span>Private target</span>';
    controller.anchor('applied');
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    let applied = false;
    controller.registerAnchorProbe(() => applied ? { kind: 'target', messageId: 'target', disposition: 'applied' } : { kind: 'pending' });
    now = 90;
    controller.resume('pwa_resume');
    expect(controller.openId).toBeDefined();
    await Promise.resolve();
    controller.inspect();
    expect(send).toHaveBeenCalledTimes(1);
    applied = true;
    target.hidden = true;
    controller.inspect();
    expect(send).toHaveBeenCalledTimes(1);
    target.hidden = false;
    now = 99;
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(payload(1)).toMatchObject({ outcome: 'ready', marks: { anchor_settled: 9 } });
  });

  it.each(['target_missing', 'superseded'] as const)('preserves current %s on resume instead of recycling successful settlement', async disposition => {
    const { root, controller } = view();
    controller.expectTarget(true, 'target');
    hydrate(root, controller);
    enable(root);
    const target = root.querySelector('article')!;
    target.id = 'message-target';
    target.innerHTML = '<span>Private target</span>';
    controller.anchor('applied');
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    controller.registerAnchorProbe(() => ({ kind: 'target', messageId: 'target', disposition }));
    now = 90;
    controller.resume('back_forward');
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(payload(1)).toMatchObject({ outcome: 'incomplete', anchor_result: disposition });
    expect(payload(1).marks.anchor_settled).toBeNull();
    expect(JSON.stringify(payload(1))).not.toContain('Private target');
  });

  it.each(['document', 'ancestor', 'breadcrumb'] as const)('does not finish a resumed tail from cached marks while current %s is hidden or disabled', async hidden => {
    const { root, controller } = view();
    hydrate(root, controller);
    enable(root);
    let settled = false;
    controller.registerAnchorProbe(() => ({ kind: 'tail', settled }));
    controller.finish('incomplete');
    await vi.advanceTimersByTimeAsync(0);
    controller.resume('pwa_resume');
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    const button = root.querySelector('[role="button"]')!;
    if (hidden === 'document') {
      Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' });
      controller.visibility(false);
    } else if (hidden === 'ancestor') root.setAttribute('aria-hidden', 'true');
    else button.setAttribute('aria-disabled', 'true');
    settled = true;
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    if (hidden === 'document') {
      Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
      controller.visibility(true);
    } else if (hidden === 'ancestor') root.removeAttribute('aria-hidden');
    else button.removeAttribute('aria-disabled');
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(payload(1)).toMatchObject({ outcome: 'ready', trigger: 'pwa_resume' });
  });

  it('does not reuse a prior visible target while waiting for breadcrumb enablement', async () => {
    const { root, controller } = view();
    controller.expectTarget(true, 'target');
    hydrate(root, controller);
    const target = root.querySelector('article')!;
    target.id = 'message-target';
    target.innerHTML = '<span>Private target</span>';
    controller.anchor('applied');
    controller.inspect();
    target.hidden = true;
    root.querySelector('#messages')!.insertAdjacentHTML('beforeend', '<div data-render-unit-key="other"><article>Other message</article></div>');
    enable(root);
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();
    target.hidden = false;
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(payload()).toMatchObject({ outcome: 'ready', anchor_result: 'applied' });
  });

  it('rechecks the owning tail after a reader leaves bottom before initial breadcrumb readiness', async () => {
    const { root, controller } = view();
    hydrate(root, controller);
    let settled = true;
    controller.registerAnchorProbe(() => ({ kind: 'tail', settled }));
    controller.tailAnchorSettled();
    controller.inspect();
    settled = false;
    enable(root);
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();
    settled = true;
    controller.inspect();
    await vi.advanceTimersByTimeAsync(0);
    expect(payload()).toMatchObject({ outcome: 'ready' });
  });

  it('expires unfinished views and coalesces inspection work, with no observer after completion', async () => {
    const { controller } = view();
    for (let i = 0; i < 1000; i++) controller.scheduleInspection();
    expect(requestAnimationFrame).toHaveBeenCalledTimes(1);
    now = 300_000;
    await vi.advanceTimersByTimeAsync(300_000);
    expect(payload()).toMatchObject({ outcome: 'incomplete', total_ms: 300_000, marks: { meaningful_transcript: null } });
    controller.scheduleInspection();
    expect(requestAnimationFrame).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledTimes(1);
  });

  it('emits an independent SSE owner edge only for a correlation accepted by the active sampled lifecycle', () => {
    const dispatch = vi.spyOn(window, 'dispatchEvent');
    const reporter = new ViewJourneyReporter({ sampleRate: 1, send });
    const identity = { ...defaultViewIdentity('ordinary', 'navigation'), segment_id: segmentId };
    const controller = new ViewJourneyLifecycle(identity, reporter, () => now);
    controllers.push(controller);
    const stream = crypto.randomUUID();
    controller.streamStarted(crypto.randomUUID(), crypto.randomUUID(), 0);
    controller.streamStarted(segmentId, 'PRIVATE_INVALID', 0);
    controller.streamStarted(segmentId, stream, 0);
    controller.streamStarted(segmentId, stream, 1);
    const edges = dispatch.mock.calls.map(([event]) => (event as CustomEvent).detail).filter(event => event?.kind === 'sse');
    expect(edges).toEqual([{ version: 1, kind: 'sse', open_id: controller.openId, stream_id: stream }]);
    controller.finish('canceled');
    controller.streamStarted(segmentId, crypto.randomUUID(), 2);
    expect(dispatch.mock.calls.map(([event]) => (event as CustomEvent).detail).filter(event => event?.kind === 'sse')).toHaveLength(1);
    expect(payload().correlations).toContainEqual({ kind: 'sse', id: stream });
  });

  it('does not emit an SSE ownership edge for unsampled lifecycle or a correlation rejected by the cap', () => {
    const dispatch = vi.spyOn(window, 'dispatchEvent');
    const unsampled = new ViewJourneyLifecycle(defaultViewIdentity('ordinary', 'navigation'), new ViewJourneyReporter({ random: () => 0.9, send }), () => now);
    controllers.push(unsampled);
    unsampled.streamStarted(segmentId, crypto.randomUUID(), 0);
    expect(dispatch.mock.calls.map(([event]) => (event as CustomEvent).detail).filter(event => event?.kind === 'sse')).toHaveLength(0);
    const { controller } = view();
    for (let index = 0; index < 16; index++) controller.requestStarted(crypto.randomUUID());
    controller.streamStarted(segmentId, crypto.randomUUID(), 0);
    expect(dispatch.mock.calls.map(([event]) => (event as CustomEvent).detail).filter(event => event?.kind === 'sse')).toHaveLength(0);
    controller.finish('incomplete');
    expect(payload().correlations).toHaveLength(16);
    expect(payload().dropped_correlations).toBe(1);
  });

});
