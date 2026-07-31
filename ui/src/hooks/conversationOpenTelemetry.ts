interface ConversationOpenTelemetryBase {
  open_id: string;
  native_open_ms: number | null;
  total_ms: number;
  retry_attempt: number;
  visible: boolean;
  effective_type: string | null;
}

export type ConversationOpenTelemetryPayload = ConversationOpenTelemetryBase & (
  | {
      outcome: 'connected';
      init_received_ms: number;
      handler_ms: number;
    }
  | {
      outcome: 'error' | 'canceled';
      init_received_ms: number | null;
      handler_ms: null;
    }
);

type Clock = () => number;

export class ConversationOpenMeasurement {
  private readonly startedAt: number;
  private nativeOpenedAt: number | null = null;
  private initReceivedAt: number | null = null;
  private completed = false;

  constructor(
    readonly openId: string,
    private readonly retryAttempt: number,
    private readonly clock: Clock = () => performance.now(),
  ) {
    this.startedAt = clock();
  }

  nativeOpen(): void {
    if (!this.completed && this.nativeOpenedAt === null) this.nativeOpenedAt = this.clock();
  }

  initReceived(): void {
    if (!this.completed && this.initReceivedAt === null) this.initReceivedAt = this.clock();
  }

  connected(): ConversationOpenTelemetryPayload | null {
    if (this.initReceivedAt === null) return this.finishIncomplete('error');
    const common = this.finishCommon();
    if (!common) return null;
    const initReceivedMs = Math.min(
      boundedMs(this.initReceivedAt - this.startedAt),
      common.payload.total_ms,
    );
    return {
      ...common.payload,
      outcome: 'connected',
      init_received_ms: initReceivedMs,
      handler_ms: boundedMs(common.payload.total_ms - initReceivedMs),
    };
  }

  error(): ConversationOpenTelemetryPayload | null {
    return this.finishIncomplete('error');
  }

  canceled(): ConversationOpenTelemetryPayload | null {
    return this.finishIncomplete('canceled');
  }

  private finishIncomplete(
    outcome: 'error' | 'canceled',
  ): ConversationOpenTelemetryPayload | null {
    const common = this.finishCommon();
    if (!common) return null;
    return {
      ...common.payload,
      outcome,
      init_received_ms: elapsed(this.initReceivedAt, this.startedAt),
      handler_ms: null,
    };
  }

  private finishCommon(): {
    payload: ConversationOpenTelemetryBase;
    finishedAt: number;
  } | null {
    if (this.completed) return null;
    this.completed = true;
    const finishedAt = this.clock();
    return {
      finishedAt,
      payload: {
        open_id: this.openId,
        native_open_ms: elapsed(this.nativeOpenedAt, this.startedAt),
        total_ms: boundedMs(finishedAt - this.startedAt),
        retry_attempt: Math.min(this.retryAttempt, 10_000),
        visible: document.visibilityState === 'visible',
        effective_type: networkEffectiveType(),
      },
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
