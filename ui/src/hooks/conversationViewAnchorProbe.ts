import type { HistoryScrollCommand, HistoryView } from '../conversation/historyExpansion';
import { sameHistoryView, transcriptPositioningCommandKey, type TranscriptPositioningState, type TranscriptPositioningFinishResult } from '../conversation/transcriptPositioning';
import type { ScrollMachineState } from '../conversation/scrollMachine';

export type ViewAnchorObservation =
  | { kind: 'pending' }
  | { kind: 'tail'; settled: boolean }
  | { kind: 'target'; messageId: string; disposition: TranscriptPositioningFinishResult };
export type ViewAnchorProbe = () => ViewAnchorObservation;
export type ViewAnchorIntent =
  | { scope: string; command: HistoryScrollCommand }
  | { scope: string; command: null; view: HistoryView };
export type ViewAnchorTerminal = { scope: string; command: HistoryScrollCommand; result: TranscriptPositioningFinishResult };

export function observeOwningAnchor(
  intent: ViewAnchorIntent,
  terminal: ViewAnchorTerminal | null,
  positioning: TranscriptPositioningState,
  scroll: ScrollMachineState,
  scroller: HTMLElement | null,
): ViewAnchorObservation {
  if (positioning.active || positioning.phase) return { kind: 'pending' };
  if (intent.command) {
    if (!terminal || terminal.scope !== intent.scope
      || transcriptPositioningCommandKey(terminal.command) !== transcriptPositioningCommandKey(intent.command)
      || !positioning.currentView || !sameHistoryView(positioning.currentView, intent.command.view)
      || !positioning.terminalCommandKeys.has(transcriptPositioningCommandKey(intent.command))) return { kind: 'pending' };
    return { kind: 'target', messageId: intent.command.kind === 'jump_to_message' ? intent.command.targetMessageId : intent.command.messageId, disposition: terminal.result };
  }
  if (!positioning.currentView || !sameHistoryView(positioning.currentView, intent.view)
    || !scroller || scroller.clientHeight <= 0 || scroller.clientWidth <= 0
    || scroller.scrollHeight <= 0 || scroll.kind !== 'live'
    || scroll.follow.kind !== 'following' || scroll.gesture.kind !== 'idle') return { kind: 'pending' };
  const gap = scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop;
  return { kind: 'tail', settled: Number.isFinite(gap) && Math.abs(gap) <= 2 };
}
