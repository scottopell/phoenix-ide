interface ConversationOpenTelemetryCommon {
  open_id: string;
  route_resolved_ms: number | null;
  native_open_ms: number | null;
  total_ms: number;
  retry_attempt: number;
  visible: boolean;
  effective_type: string | null;
}

interface ConnectedConversationOpenTelemetry extends ConversationOpenTelemetryCommon {
  outcome: 'connected';
  event_source_created_ms: number;
  init_received_ms: number;
  init_handled_ms: number;
  first_paint_ms: number;
}

interface UnfinishedConversationOpenTelemetry extends ConversationOpenTelemetryCommon {
  outcome: 'error' | 'canceled';
  event_source_created_ms: number | null;
  init_received_ms: number | null;
  init_handled_ms: number | null;
  first_paint_ms: null;
}

export type ConversationOpenTelemetryPayload =
  | ConnectedConversationOpenTelemetry
  | UnfinishedConversationOpenTelemetry;

type Clock = () => number;

export class ConversationOpenMeasurement {
  private readonly startedAt: number;
  private routeResolvedAt: number | null = null;
  private eventSourceCreatedAt: number | null = null;
  private nativeOpenedAt: number | null = null;
  private initReceivedAt: number | null = null;
  private initHandledAt: number | null = null;
  private completed = false;
  private remainedVisible = document.visibilityState === 'visible';

  constructor(
    readonly openId: string,
    private readonly retryAttempt: number,
    private readonly clock: Clock = () => performance.now(),
    startedAt?: number,
  ) {
    this.startedAt = startedAt ?? clock();
  }

  isCompleted(): boolean {
    return this.completed;
  }

  documentHidden(): void {
    this.remainedVisible = false;
  }

  routeResolved(): void {
    if (!this.completed && this.routeResolvedAt === null) this.routeResolvedAt = this.clock();
  }

  eventSourceCreated(): void {
    if (!this.completed && this.eventSourceCreatedAt === null) this.eventSourceCreatedAt = this.clock();
  }

  nativeOpen(): void {
    if (!this.completed && this.nativeOpenedAt === null) this.nativeOpenedAt = this.clock();
  }

  initReceived(): void {
    if (!this.completed && this.initReceivedAt === null) this.initReceivedAt = this.clock();
  }

  initHandled(): void {
    if (!this.completed && this.initHandledAt === null) this.initHandledAt = this.clock();
  }

  firstPaint(): ConversationOpenTelemetryPayload | null {
    if (
      this.eventSourceCreatedAt === null
      || this.initReceivedAt === null
      || this.initHandledAt === null
    ) return this.finish('error');
    return this.finish('connected');
  }

  error(): ConversationOpenTelemetryPayload | null {
    return this.finish('error');
  }

  canceled(): ConversationOpenTelemetryPayload | null {
    return this.finish('canceled');
  }

  private finish(outcome: 'connected' | 'error' | 'canceled'): ConversationOpenTelemetryPayload | null {
    if (this.completed) return null;
    this.completed = true;
    const finishedAt = this.clock();
    const common: ConversationOpenTelemetryCommon = {
      open_id: this.openId,
      route_resolved_ms: elapsed(this.routeResolvedAt, this.startedAt),
      native_open_ms: elapsed(this.nativeOpenedAt, this.startedAt),
      total_ms: boundedMs(finishedAt - this.startedAt),
      retry_attempt: Math.min(this.retryAttempt, 10_000),
      visible: this.remainedVisible && document.visibilityState === 'visible',
      effective_type: networkEffectiveType(),
    };
    if (outcome === 'connected') {
      return {
        ...common,
        outcome,
        event_source_created_ms: elapsed(this.eventSourceCreatedAt, this.startedAt)!,
        init_received_ms: elapsed(this.initReceivedAt, this.startedAt)!,
        init_handled_ms: elapsed(this.initHandledAt, this.startedAt)!,
        first_paint_ms: boundedMs(finishedAt - this.startedAt),
      };
    }
    return {
      ...common,
      outcome,
      event_source_created_ms: elapsed(this.eventSourceCreatedAt, this.startedAt),
      init_received_ms: elapsed(this.initReceivedAt, this.startedAt),
      init_handled_ms: elapsed(this.initHandledAt, this.startedAt),
      first_paint_ms: null,
    };
  }
}

function elapsed(end: number | null, start: number | null): number | null {
  return end === null || start === null ? null : boundedMs(end - start);
}

function boundedMs(value: number): number {
  return Math.max(0, Math.min(300_000, Math.round(value * 10) / 10));
}

const NETWORK_EFFECTIVE_TYPES = new Set(['slow-2g', '2g', '3g', '4g']);

function networkEffectiveType(): string | null {
  const connection = (navigator as Navigator & {
    connection?: { effectiveType?: string };
  }).connection;
  const effectiveType = connection?.effectiveType;
  return effectiveType && NETWORK_EFFECTIVE_TYPES.has(effectiveType) ? effectiveType : null;
}

export function reportConversationOpen(payload: ConversationOpenTelemetryPayload): void {
  void fetch('/api/telemetry/conversation-open', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
    keepalive: true,
  }).catch(() => undefined);
}
