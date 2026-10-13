import type { ViewAnchorProbe } from './conversationViewAnchorProbe';
import type { AnchorResult, ConversationViewJourney, JourneyOutcome, JourneyTrigger } from './conversationViewJourney';
import type { ViewResponseMetadata } from './conversationViewResponse';
export interface ViewRequestObservation {
  response: (metadata: ViewResponseMetadata) => void;
  snapshot: (productId: string, segmentId: string) => void;
  failed: () => void;
}
import { ViewJourneyReporter, countBucket, type SampledViewJourney, type ViewIdentity } from './conversationViewTelemetry';

export function isCommitVisible(element: Element | null, clip?: Element | null): boolean {
  if (!element?.isConnected || document.visibilityState !== 'visible') return false;
  let ancestor: Element | null = element;
  for (let depth = 0; ancestor && depth < 64; depth++, ancestor = ancestor.parentElement) {
    if (ancestor.hasAttribute('hidden') || ancestor.getAttribute('aria-hidden') === 'true') return false;
    const style = getComputedStyle(ancestor);
    if (style.display === 'none' || style.visibility === 'hidden' || style.visibility === 'collapse' || style.opacity === '0') return false;
  }
  if (ancestor !== null) return false;
  const rect = element.getBoundingClientRect();
  const bounds = clip?.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0
    && Math.min(rect.right, bounds?.right ?? window.innerWidth, window.innerWidth) > Math.max(rect.left, bounds?.left ?? 0, 0)
    && Math.min(rect.bottom, bounds?.bottom ?? window.innerHeight, window.innerHeight) > Math.max(rect.top, bounds?.top ?? 0, 0);
}

function anyVisible(root: Element, selector: string, clip: Element, measurement: ConversationViewJourney): boolean {
  const candidates = root.querySelectorAll(selector);
  if (candidates.length > 64) measurement.observerScanLimited();
  for (let index = 0; index < Math.min(64, candidates.length); index++) {
    if (isCommitVisible(candidates[index] ?? null, clip)) return true;
  }
  return false;
}

export class ViewJourneyLifecycle {
  private attempt: SampledViewJourney | null | undefined;
  private finished = false;
  private root: HTMLElement | null = null;
  private observer: MutationObserver | null = null;
  private frame = 0;
  private deadline: ReturnType<typeof setTimeout> | undefined;
  private hydrated = false;
  private tailSettled = false;
  private needsTarget = false;
  private targetMessageId: string | null = null;
  private targetApplied = false;
  private anchorProbe: ViewAnchorProbe | null = null;
  private resumed = false;
  private transcriptSeen = false;
  private breadcrumbSeen = false;
  private breadcrumbEnabled = false;
  private anchorSeen = false;
  private readonly reporter: ViewJourneyReporter;
  private readonly clock: () => number;
  private identity: ViewIdentity;

  constructor(identity: ViewIdentity, reporter: ViewJourneyReporter, clock = () => performance.now()) {
    this.identity = identity;
    this.reporter = reporter;
    this.clock = clock;
  }

  get openId(): string | undefined {
    return this.finished ? undefined : this.ensure()?.openId;
  }

  private ensure(): SampledViewJourney | null {
    if (this.attempt === undefined && !this.finished) {
      this.attempt = this.reporter.begin(this.identity, this.clock);
      if (this.attempt) this.deadline = setTimeout(() => this.finish('incomplete'), 300_000);
      if (this.attempt && this.identity.cache === 'retained') {
        if (this.hydrated) this.attempt.measurement.mark('hydration_ready');
        if (this.identity.chapter_count_bucket !== 'unknown') this.attempt.measurement.mark('chapter_data_ready');
      }
    }
    return this.attempt ?? null;
  }

  attach = (root: HTMLElement | null): void => {
    this.disconnect();
    this.root = root;
    if (!root || this.finished || !this.ensure()) return;
    this.ensure()?.measurement.mark('shell');
    this.observer = new MutationObserver(() => this.scheduleInspection());
    this.observer.observe(root, { subtree: true, childList: true, attributes: true, attributeFilter: ['hidden', 'aria-hidden', 'aria-disabled', 'disabled', 'class', 'style'] });
    root.addEventListener('transitionend', this.scheduleInspection, { passive: true });
    root.addEventListener('animationend', this.scheduleInspection, { passive: true });
    this.scheduleInspection();
  };

  registerAnchorProbe = (probe: ViewAnchorProbe | null): void => { this.anchorProbe = probe; };

  expectTarget(required: boolean, messageId: string | null = null): void {
    if (this.targetMessageId !== messageId) this.targetApplied = false;
    this.needsTarget = required;
    this.targetMessageId = messageId;
  }

  requestStarted(requestId: string): ViewRequestObservation | undefined {
    const attempt = this.ensure();
    if (!attempt || this.finished) return undefined;
    attempt.identity.cache = 'network';
    attempt.measurement.correlate('request', requestId);
    const owns = () => this.attempt === attempt && !this.finished;
    return {
      response: metadata => { if (owns()) this.responseObserved(metadata); },
      snapshot: (productId, segmentId) => { if (owns()) this.snapshotReceived(productId, segmentId); },
      failed: () => { if (owns()) this.finish('failed'); },
    };
  }

  streamStarted = (conversationId: string, openId: string, retryAttempt: number): void => {
    const attempt = this.ensure();
    if (!attempt || this.finished) return;
    if (this.identity.segment_id !== null && this.identity.segment_id !== conversationId) return;
    attempt.measurement.correlate('sse', openId);
    attempt.identity.reconnect_count = Math.min(10_000, retryAttempt);
  };

  responseObserved = (metadata: ViewResponseMetadata): void => {
    const attempt = this.ensure();
    if (!attempt || this.finished) return;
    attempt.identity.response_size_bucket = metadata.size_bucket;
    if (metadata.trace_id) attempt.measurement.correlate('trace', metadata.trace_id);
  };

  snapshotReceived(productId: string, segmentId: string): void {
    this.identity.product_conversation_id = productId;
    this.identity.segment_id = segmentId;
    const attempt = this.ensure();
    if (!attempt || this.finished) return;
    attempt.identity.product_conversation_id = productId;
    attempt.identity.segment_id = segmentId;
    attempt.measurement.mark('snapshot_received');
  }

  hydrationReady(productId: string, segmentId: string, count: number): void {
    this.identity.product_conversation_id = productId;
    this.identity.segment_id = segmentId;
    this.identity.message_count_bucket = countBucket(count);
    if (this.identity.cache === 'unknown') this.identity.cache = 'retained';
    this.hydrated = true;
    const attempt = this.ensure();
    if (!attempt || this.finished) return;
    attempt.identity.product_conversation_id = productId;
    attempt.identity.segment_id = segmentId;
    attempt.identity.message_count_bucket = countBucket(count);
    if (attempt.identity.cache === 'unknown') attempt.identity.cache = 'retained';
    attempt.measurement.mark('hydration_ready');
    this.scheduleInspection();
  }

  chaptersReady = (count: number): void => {
    this.identity.chapter_count_bucket = countBucket(count);
    const attempt = this.ensure();
    if (!attempt || this.finished) return;
    attempt.identity.chapter_count_bucket = countBucket(count);
    attempt.measurement.mark('chapter_data_ready');
    this.scheduleInspection();
  };

  tailAnchorSettled = (): void => {
    if (this.finished) return;
    this.tailSettled = true;
    this.scheduleInspection();
  };

  anchor(result: AnchorResult): void {
    const attempt = this.ensure();
    if (!attempt || this.finished) return;
    if (result === 'target_missing' || result === 'superseded') {
      attempt.measurement.anchor(result);
      this.finish('incomplete');
    } else if (result === 'applied') {
      this.targetApplied = true;
      this.scheduleInspection();
    }
  }

  visibility(visible: boolean): void {
    if (this.finished) return;
    this.ensure()?.measurement.visibility(visible);
    if (visible) this.scheduleInspection();
  }

  resume(trigger: JourneyTrigger = 'pwa_resume'): void {
    this.resumed = true;
    this.finish('incomplete');
    this.identity = { ...this.identity, trigger, visible: true, cache: 'retained', response_size_bucket: 'unknown' };
    this.attempt = undefined;
    this.finished = false;
    this.tailSettled = false;
    this.targetApplied = false;
    this.transcriptSeen = this.breadcrumbSeen = this.breadcrumbEnabled = this.anchorSeen = false;
    this.attach(this.root);
  }

  finish(outcome: JourneyOutcome): void {
    if (this.finished) return;
    const attempt = this.ensure();
    this.finished = true;
    if (this.deadline !== undefined) clearTimeout(this.deadline);
    this.deadline = undefined;
    this.disconnect();
    if (attempt) this.reporter.report(attempt, outcome);
  }

  private disconnect(): void {
    this.root?.removeEventListener('transitionend', this.scheduleInspection);
    this.root?.removeEventListener('animationend', this.scheduleInspection);
    this.observer?.disconnect();
    this.observer = null;
    if (this.frame) cancelAnimationFrame(this.frame);
    this.frame = 0;
  }

  scheduleInspection = (): void => {
    if (this.frame || this.finished || !this.attempt || !this.root) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      try { this.inspect(); } catch { this.finish('incomplete'); }
    });
  };

  inspect(): void {
    const attempt = this.attempt;
    const root = this.root;
    if (!attempt || !root || this.finished) return;
    attempt.measurement.observerInspection();
    const transcript = root.querySelector('#messages');
    if (!this.transcriptSeen && this.hydrated && isCommitVisible(transcript)) {
      if (transcript && anyVisible(transcript, '[data-render-unit-key] > *, .empty-state', transcript, attempt.measurement)) {
        attempt.measurement.mark('meaningful_transcript');
        this.transcriptSeen = true;
      }
    }
    const nav = root.querySelector('#conversation-nav-trail');
    if ((!this.breadcrumbSeen || !this.breadcrumbEnabled) && isCommitVisible(nav)) {
      if (nav && anyVisible(nav, '.conversation-nav-item', nav, attempt.measurement)) {
        attempt.measurement.mark('breadcrumb_presented');
        this.breadcrumbSeen = true;
      }
      if (nav && anyVisible(nav, '.conversation-nav-item[role="button"]:not([aria-disabled="true"]):not([disabled])', nav, attempt.measurement)) {
        attempt.measurement.mark('breadcrumb_enabled');
        this.breadcrumbEnabled = true;
      }
    }
    if (this.resumed && !this.anchorSeen && this.anchorProbe) {
      const observation = this.anchorProbe();
      this.tailSettled = observation.kind === 'tail' && observation.settled;
      this.targetApplied = observation.kind === 'target' && observation.messageId === this.targetMessageId && observation.disposition === 'applied';
      if (observation.kind === 'target' && observation.messageId === this.targetMessageId && observation.disposition !== 'applied') {
        attempt.measurement.anchor(observation.disposition);
        this.finish('incomplete');
        return;
      }
    }
    if (this.needsTarget && !this.anchorSeen && this.targetApplied && this.transcriptSeen && transcript && this.targetMessageId !== null) {
      const target = document.getElementById(`message-${this.targetMessageId}`);
      if (target && transcript.contains(target) && isCommitVisible(target, transcript)
        && anyVisible(target, ':scope > *', transcript, attempt.measurement)) {
        attempt.measurement.anchor('applied');
        this.anchorSeen = true;
      }
    }
    if (!this.needsTarget && !this.anchorSeen && this.transcriptSeen && this.tailSettled) {
      attempt.measurement.anchor('tail_pinned');
      this.anchorSeen = true;
    }
    if (!this.needsTarget && !this.anchorSeen && this.transcriptSeen && transcript?.querySelector('.empty-state')) {
      attempt.measurement.anchor('empty');
      this.anchorSeen = true;
    }
    if (this.transcriptSeen && this.breadcrumbSeen && this.breadcrumbEnabled && this.anchorSeen) this.finish('ready');
  }
}

export function defaultViewIdentity(route: ViewIdentity['route'], trigger: JourneyTrigger): ViewIdentity {
  const ua = navigator.userAgent;
  const standalone = matchMedia('(display-mode: standalone)').matches || (navigator as Navigator & { standalone?: boolean }).standalone === true;
  return {
    route, trigger, product_conversation_id: null, segment_id: null, standalone,
    platform: /iPad|iPhone|iPod/.test(ua) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1) ? 'ios' : /Android/.test(ua) ? 'android' : /Mac|Windows|Linux/.test(ua) ? 'desktop' : 'unknown',
    frontend_source: typeof __PHOENIX_UI_SOURCE__ === 'undefined' ? null : __PHOENIX_UI_SOURCE__,
    frontend_source_dirty: typeof __PHOENIX_UI_SOURCE_DIRTY__ === 'undefined' || __PHOENIX_UI_SOURCE_DIRTY__,
    visible: document.visibilityState === 'visible', cache: 'unknown',
    response_size_bucket: 'unknown',
    message_count_bucket: 'unknown', chapter_count_bucket: 'unknown', reconnect_count: 0,
  };
}
