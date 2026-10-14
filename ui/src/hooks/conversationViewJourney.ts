export const JOURNEY_MARKS = [
  'shell', 'snapshot_received', 'hydration_ready', 'meaningful_transcript',
  'chapter_data_ready', 'breadcrumb_presented', 'breadcrumb_enabled', 'anchor_settled',
] as const;
const observableMarks = JOURNEY_MARKS.filter(mark => mark !== 'anchor_settled');
export type JourneyMark = typeof JOURNEY_MARKS[number];
export type JourneyOutcome = 'ready' | 'failed' | 'canceled' | 'incomplete';
export type JourneyTrigger = 'navigation' | 'reload' | 'back_forward' | 'pwa_resume';
export type CorrelationKind = 'request' | 'sse' | 'trace';
export type AnchorResult = 'applied' | 'tail_pinned' | 'empty' | 'target_missing' | 'superseded';
export interface JourneyReport {
  marks: Record<JourneyMark, number | null>;
  outcome: JourneyOutcome;
  total_ms: number;
  timeline_saturated: boolean;
  background_ms: number;
  background_interval_count: number;
  background_open: boolean;
  observer_inspection_count: number;
  observer_scan_limited: boolean;
  anchor_result: AnchorResult | null;
  breadcrumb_ready_minus_meaningful_ms: number | null;
  presentation_evidence: 'commit_frame_estimate';
  correlations: { kind: CorrelationKind; id: string }[];
  dropped_correlations: number;
}
const MAX_MS = 300_000;
const MAX_CORRELATIONS = 16;
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const traceId = /^[0-9a-f]{32}$/i;

export class ConversationViewJourney {
  readonly #clock: () => number;
  readonly #startedAt: number;
  readonly #marks = Object.fromEntries(JOURNEY_MARKS.map(mark => [mark, null])) as Record<JourneyMark, number | null>;
  readonly #correlations: JourneyReport['correlations'] = [];
  #droppedCorrelations = 0;
  #backgroundStartedAt: number | null = null;
  #backgroundMs = 0;
  #backgroundIntervals = 0;
  #inspections = 0;
  #scanLimited = false;
  #finished = false;
  #saturated = false;
  #anchorResult: AnchorResult | null = null;

  constructor(clock: () => number = () => performance.now()) {
    this.#clock = clock;
    this.#startedAt = clock();
    Object.freeze(this);
  }

  mark(mark: Exclude<JourneyMark, 'anchor_settled'>): void {
    if (!this.#finished && observableMarks.includes(mark) && this.#marks[mark] === null) this.#marks[mark] = this.#elapsed();
  }

  anchor(result: AnchorResult): void {
    if (this.#finished || this.#anchorResult !== null || !['applied', 'tail_pinned', 'empty', 'target_missing', 'superseded'].includes(result)) return;
    this.#anchorResult = result;
    if (result === 'applied' || result === 'tail_pinned' || result === 'empty') this.#marks.anchor_settled = this.#elapsed();
  }

  correlate(kind: CorrelationKind, id: string): void {
    if (this.#finished || !['request', 'sse', 'trace'].includes(kind)) return;
    if (!(kind === 'trace' ? traceId : uuid).test(id)) return;
    if (this.#correlations.some(item => item.kind === kind && item.id === id)) return;
    if (this.#correlations.length === MAX_CORRELATIONS) {
      this.#droppedCorrelations = Math.min(10_000, this.#droppedCorrelations + 1);
      return;
    }
    this.#correlations.push({ kind, id });
  }

  observerInspection(): void {
    if (!this.#finished) this.#inspections = Math.min(10_000, this.#inspections + 1);
  }

  observerScanLimited(): void { if (!this.#finished) this.#scanLimited = true; }

  visibility(visible: boolean): void {
    if (this.#finished) return;
    const now = this.#elapsed();
    if (!visible && this.#backgroundStartedAt === null) {
      this.#backgroundStartedAt = now;
      this.#backgroundIntervals = Math.min(10_000, this.#backgroundIntervals + 1);
    } else if (visible && this.#backgroundStartedAt !== null) {
      this.#backgroundMs += now - this.#backgroundStartedAt;
      this.#backgroundStartedAt = null;
    }
  }

  finish(outcome: JourneyOutcome): JourneyReport | null {
    if (this.#finished) return null;
    const total = this.#elapsed();
    const terminal = ['ready', 'failed', 'canceled', 'incomplete'].includes(outcome) ? outcome : 'incomplete';
    const { meaningful_transcript: transcript, breadcrumb_presented: presented, breadcrumb_enabled: enabled } = this.#marks;
    const ready = transcript !== null && presented !== null && enabled !== null && this.#marks.anchor_settled !== null;
    this.#finished = true;
    return {
      marks: { ...this.#marks },
      outcome: terminal === 'ready' && (!ready || this.#saturated) ? 'incomplete' : terminal,
      total_ms: total,
      timeline_saturated: this.#saturated,
      anchor_result: this.#anchorResult,
      background_interval_count: this.#backgroundIntervals,
      background_open: this.#backgroundStartedAt !== null,
      observer_inspection_count: this.#inspections,
      observer_scan_limited: this.#scanLimited,
      background_ms: Math.min(total, this.#backgroundMs + (this.#backgroundStartedAt === null ? 0 : total - this.#backgroundStartedAt)),
      breadcrumb_ready_minus_meaningful_ms: this.#saturated || transcript === null || presented === null || enabled === null ? null : Math.max(presented, enabled) - transcript,
      presentation_evidence: 'commit_frame_estimate',
      correlations: this.#correlations.map(item => ({ ...item })),
      dropped_correlations: this.#droppedCorrelations,
    };
  }

  #elapsed(): number {
    const raw = this.#clock() - this.#startedAt;
    this.#saturated ||= raw > MAX_MS;
    return Math.max(0, Math.min(MAX_MS, raw));
  }
}
