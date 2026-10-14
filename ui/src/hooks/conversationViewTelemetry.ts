import { emitViewDiagnostic } from './conversationViewDiagnostics';
import { ConversationViewJourney, type JourneyReport, type JourneyTrigger } from './conversationViewJourney';
import type { ResponseSizeBucket } from './conversationViewResponse';
import { generateUUID } from '../utils/uuid';

export type CountBucket = 'unknown' | '0' | '1-10' | '11-100' | '101-1000' | '1001+';
export function countBucket(count: number): CountBucket {
  if (!Number.isInteger(count) || count < 0) return 'unknown';
  return count === 0 ? '0' : count <= 10 ? '1-10' : count <= 100 ? '11-100' : count <= 1000 ? '101-1000' : '1001+';
}

export interface ViewIdentity {
  product_conversation_id: string | null;
  segment_id: string | null;
  route: 'global' | 'ordinary';
  trigger: JourneyTrigger;
  standalone: boolean;
  platform: 'ios' | 'android' | 'desktop' | 'unknown';
  frontend_source: string | null;
  frontend_source_dirty: boolean;
  cache: 'network' | 'retained' | 'unknown';
  message_count_bucket: CountBucket;
  chapter_count_bucket: CountBucket;
  response_size_bucket: ResponseSizeBucket;
  reconnect_count: number;
  visible: boolean;
}
export interface ViewJourneyPayload extends ViewIdentity, JourneyReport {
  open_id: string;
  reporter_id: string;
  counters_saturated: boolean;
  sample_rate: number;
  admitted_count: number;
  sampled_count: number;
  transport_dropped_count: number;
  transport_failed_count: number;
}
class SampledRecord {
  readonly #owner: object;
  readonly openId: string;
  readonly measurement: ConversationViewJourney;
  readonly identity: ViewIdentity;

  constructor(owner: object, identity: ViewIdentity, measurement: ConversationViewJourney, openId: string) {
    this.openId = openId;
    this.#owner = owner;
    this.identity = identity;
    this.measurement = measurement;
    Object.freeze(this);
  }

  ownedBy(owner: object): boolean { return owner === this.#owner; }
}
export type SampledViewJourney = SampledRecord;
const MAX_COUNT = 10_000;
const increment = (n: number) => Math.min(MAX_COUNT, n + 1);
const opaqueId = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const sourceId = /^[0-9a-f]{7,40}$/i;

function cleanIdentity(identity: ViewIdentity): ViewIdentity {
  return {
    product_conversation_id: identity.product_conversation_id && opaqueId.test(identity.product_conversation_id) ? identity.product_conversation_id : null,
    segment_id: identity.segment_id && opaqueId.test(identity.segment_id) ? identity.segment_id : null,
    frontend_source: identity.frontend_source && sourceId.test(identity.frontend_source) ? identity.frontend_source : null,
    frontend_source_dirty: identity.frontend_source_dirty === true,
    cache: ['network', 'retained', 'unknown'].includes(identity.cache) ? identity.cache : 'unknown',
    message_count_bucket: ['unknown', '0', '1-10', '11-100', '101-1000', '1001+'].includes(identity.message_count_bucket) ? identity.message_count_bucket : 'unknown',
    chapter_count_bucket: ['unknown', '0', '1-10', '11-100', '101-1000', '1001+'].includes(identity.chapter_count_bucket) ? identity.chapter_count_bucket : 'unknown',
    response_size_bucket: ['unknown', '0-10k', '10k-100k', '100k-1m', '1m+'].includes(identity.response_size_bucket) ? identity.response_size_bucket : 'unknown',
    reconnect_count: Number.isFinite(identity.reconnect_count) ? Math.min(10_000, Math.max(0, Math.floor(identity.reconnect_count))) : 0,
    route: identity.route === 'global' ? 'global' : 'ordinary',
    trigger: ['navigation', 'reload', 'back_forward', 'pwa_resume'].includes(identity.trigger) ? identity.trigger : 'navigation',
    standalone: identity.standalone === true,
    platform: ['ios', 'android', 'desktop', 'unknown'].includes(identity.platform) ? identity.platform : 'unknown',
    visible: identity.visible === true,
  };
}

export class ViewJourneyReporter {
  readonly #owner = {};
  readonly #reporterId = generateUUID();
  #admitted = 0;
  #sampled = 0;
  #dropped = 0;
  #failed = 0;
  #busy = false;
  readonly #sampleRate: number;
  readonly #random: () => number;
  readonly #send: typeof fetch;

  constructor(options: { sampleRate?: number; random?: () => number; send?: typeof fetch } = {}) {
    this.#sampleRate = options.sampleRate ?? 0.25;
    if (!(this.#sampleRate > 0 && this.#sampleRate <= 1)) throw new Error('Invalid journey sampling rate');
    this.#random = options.random ?? Math.random;
    this.#send = options.send ?? fetch;
    Object.freeze(this);
  }

  begin(identity: ViewIdentity, clock?: () => number): SampledViewJourney | null {
    this.#admitted = increment(this.#admitted);
    if (this.#random() >= this.#sampleRate) {
      emitViewDiagnostic({ kind: 'admission', route: identity.route === 'global' ? 'global' : 'ordinary', open_id: null, sampled: false });
      return null;
    }
    this.#sampled = increment(this.#sampled);
    const clean = cleanIdentity(identity);
    const openId = generateUUID();
    emitViewDiagnostic({ kind: 'admission', route: clean.route, open_id: openId, sampled: true });
    const measurement = new ConversationViewJourney(clock, event => emitViewDiagnostic({ ...event, open_id: openId }));
    measurement.visibility(clean.visible);
    return new SampledRecord(this.#owner, clean, measurement, openId);
  }

  report(journey: SampledViewJourney, outcome: JourneyReport['outcome']): void {
    if (!journey.ownedBy(this.#owner)) {
      this.#dropped = increment(this.#dropped);
      return;
    }
    const report = journey.measurement.finish(outcome);
    if (!report) return;
    emitViewDiagnostic({ kind: 'terminal', open_id: journey.openId, outcome: report.outcome });
    if (!opaqueId.test(journey.openId)) {
      this.#dropped = increment(this.#dropped);
      return;
    }
    if (this.#busy) {
      emitViewDiagnostic({ kind: 'submission', open_id: journey.openId, state: 'busy_drop', status: null });
      this.#dropped = increment(this.#dropped);
      return;
    }
    const identity = cleanIdentity(journey.identity);
    const payload: ViewJourneyPayload = {
      ...identity, ...report, open_id: journey.openId,
      reporter_id: this.#reporterId,
      counters_saturated: [this.#admitted, this.#sampled, this.#dropped, this.#failed, report.dropped_correlations, report.background_interval_count, report.observer_inspection_count, identity.reconnect_count].includes(MAX_COUNT),
      sample_rate: this.#sampleRate, admitted_count: this.#admitted, sampled_count: this.#sampled,
      transport_dropped_count: this.#dropped, transport_failed_count: this.#failed,
    };
    this.#busy = true;
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 3000);
    void Promise.resolve().then(() => {
      emitViewDiagnostic({ kind: 'submission', open_id: journey.openId, state: 'attempted', status: null });
      return this.#send('/api/telemetry/conversation-view', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload), keepalive: true, signal: controller.signal,
      });
    }).then(response => {
      emitViewDiagnostic({ kind: 'submission', open_id: journey.openId, state: 'response', status: response.status });
      if (!response.ok) this.#failed = increment(this.#failed);
    }).catch(() => {
      emitViewDiagnostic({ kind: 'submission', open_id: journey.openId, state: 'failed', status: null });
      this.#failed = increment(this.#failed);
    }).finally(() => {
      clearTimeout(timeout);
      this.#busy = false;
    });
  }
}
