export type ResponseSizeBucket = 'unknown' | '0-10k' | '10k-100k' | '100k-1m' | '1m+';
export interface ViewResponseMetadata { size_bucket: ResponseSizeBucket; trace_id: string | null }
export type ViewResponseObserver = (metadata: ViewResponseMetadata) => void;

export function observeViewResponse(response: Response, observer?: ViewResponseObserver): void {
  if (!observer) return;
  const length = response.headers.get('content-length');
  const bytes = length && /^\d{1,12}$/.test(length) ? Number(length) : null;
  const trace = response.headers.get('traceparent')?.match(/^00-([0-9a-f]{32})-([0-9a-f]{16})-[0-9a-f]{2}$/i);
  const traceId = trace && !/^0{32}$/.test(trace[1]!) && !/^0{16}$/.test(trace[2]!) ? trace[1]!.toLowerCase() : null;
  try {
    observer({
      size_bucket: bytes === null ? 'unknown' : bytes <= 10_000 ? '0-10k' : bytes <= 100_000 ? '10k-100k' : bytes <= 1_000_000 ? '100k-1m' : '1m+',
      trace_id: traceId,
    });
  } catch {
    console.debug('View telemetry response observation unavailable');
  }
}
