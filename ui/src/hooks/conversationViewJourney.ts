export const JOURNEY_MARKS = [
  'shell', 'snapshot_received', 'hydration_ready', 'meaningful_transcript',
  'chapter_data_ready', 'breadcrumb_presented', 'breadcrumb_enabled', 'anchor_settled',
] as const;
export type JourneyMark = typeof JOURNEY_MARKS[number];
export type JourneyOutcome = 'ready' | 'failed' | 'canceled' | 'incomplete';
export type JourneyTrigger = 'navigation' | 'reload' | 'back_forward' | 'pwa_resume';
export type CorrelationKind = 'request' | 'sse' | 'trace';
export interface JourneyReport {
  marks: Record<JourneyMark, number | null>;
  outcome: JourneyOutcome;
  total_ms: number;
  timeline_saturated: boolean;
  background_ms: number;
  breadcrumb_ready_minus_meaningful_ms: number | null;
  presentation_evidence: 'react_commit_estimate';
  correlations: { kind: CorrelationKind; id: string }[];
  dropped_correlations: number;
}
const MAX_MS = 300_000;
const MAX_CORRELATIONS = 16;
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const traceId = /^[0-9a-f]{32}$/i;

export class ConversationViewJourney {
  private readonly clock: () => number;
  private readonly startedAt: number;
  private readonly marks = Object.fromEntries(JOURNEY_MARKS.map(mark => [mark, null])) as Record<JourneyMark, number | null>;
  private readonly correlations: JourneyReport['correlations'] = [];
  private droppedCorrelations = 0;
  private backgroundStartedAt: number | null = null;
  private backgroundMs = 0;
  private finished = false;
  private saturated = false;

  constructor(clock: () => number = () => performance.now()) {
    this.clock = clock;
    this.startedAt = clock();
  }

  mark(mark: JourneyMark): void {
    if (!this.finished && this.marks[mark] === null) this.marks[mark] = this.elapsed();
  }

  correlate(kind: CorrelationKind, id: string): void {
    if (this.finished) return;
    if (!(kind === 'trace' ? traceId : uuid).test(id)) return;
    if (this.correlations.some(item => item.kind === kind && item.id === id)) return;
    if (this.correlations.length === MAX_CORRELATIONS) {
      this.droppedCorrelations = Math.min(10_000, this.droppedCorrelations + 1);
      return;
    }
    this.correlations.push({ kind, id });
  }

  visibility(visible: boolean): void {
    if (this.finished) return;
    const now = this.elapsed();
    if (!visible) this.backgroundStartedAt ??= now;
    else if (this.backgroundStartedAt !== null) {
      this.backgroundMs += now - this.backgroundStartedAt;
      this.backgroundStartedAt = null;
    }
  }

  finish(outcome: JourneyOutcome): JourneyReport | null {
    if (this.finished) return null;
    const total = this.elapsed();
    const { meaningful_transcript: transcript, breadcrumb_presented: presented, breadcrumb_enabled: enabled } = this.marks;
    const ready = transcript !== null && presented !== null && enabled !== null && this.marks.anchor_settled !== null;
    this.finished = true;
    return {
      marks: { ...this.marks },
      outcome: outcome === 'ready' && (!ready || this.saturated) ? 'incomplete' : outcome,
      total_ms: total,
      timeline_saturated: this.saturated,
      background_ms: Math.min(total, this.backgroundMs + (this.backgroundStartedAt === null ? 0 : total - this.backgroundStartedAt)),
      breadcrumb_ready_minus_meaningful_ms: this.saturated || transcript === null || presented === null || enabled === null ? null : Math.max(presented, enabled) - transcript,
      presentation_evidence: 'react_commit_estimate',
      correlations: this.correlations.map(item => ({ ...item })),
      dropped_correlations: this.droppedCorrelations,
    };
  }

  private elapsed(): number {
    const raw = this.clock() - this.startedAt;
    this.saturated ||= raw > MAX_MS;
    return Math.max(0, Math.min(MAX_MS, raw));
  }
}
