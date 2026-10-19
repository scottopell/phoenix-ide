import type { AnchorResult, JourneyMark, JourneyOutcome } from './conversationViewJourney';

export type ViewDiagnostic =
  | { kind: 'admission'; route: 'ordinary' | 'global'; open_id: string | null; sampled: boolean }
  | { kind: 'mark'; open_id: string; mark: JourneyMark; ms: number }
  | { kind: 'request'; open_id: string; request_id: string }
  | { kind: 'sse'; open_id: string; stream_id: string }
  | { kind: 'anchor'; open_id: string; result: AnchorResult }
  | { kind: 'terminal'; open_id: string; outcome: JourneyOutcome }
  | { kind: 'submission'; open_id: string; state: 'attempted' | 'timeout_abort' | 'busy_drop' | 'response' | 'failed'; status: number | null };

export function emitViewDiagnostic(detail: ViewDiagnostic): void {
  if (typeof window === 'undefined') return;
  try {
    window.dispatchEvent(new CustomEvent('phoenix:view-diagnostic', { detail: Object.freeze({ version: 1, ...detail }) }));
  } catch { /* Diagnostic delivery cannot change journey or transport behavior. */ }
}
