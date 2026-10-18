import { describe, expect, it } from 'vitest';
import type { HistoryScrollCommand } from '../conversation/historyExpansion';
import { initialTranscriptPositioningState, transcriptPositioningCommandKey } from '../conversation/transcriptPositioning';
import type { ScrollMachineState } from '../conversation/scrollMachine';
import { observeOwningAnchor, type ViewAnchorIntent, type ViewAnchorTerminal } from './conversationViewAnchorProbe';

const command: HistoryScrollCommand = { kind: 'jump_to_message', token: 4, requestToken: 0, view: { conversationId: 'product', generation: 2, transcriptGeneration: 3 }, targetMessageId: 'target' };
const intent: ViewAnchorIntent = { scope: '4:segment:2', command };
const terminal: ViewAnchorTerminal = { scope: intent.scope, command, result: 'applied' };
const state = { ...initialTranscriptPositioningState(command.view), terminalCommandKeys: new Set([transcriptPositioningCommandKey(command)]) };
const scroll: Extract<ScrollMachineState, { kind: 'live' }> = { kind: 'live', conversationId: 'product', follow: { kind: 'following' }, gesture: { kind: 'idle' }, geometry: { atBottom: true, lastSnapshot: null, previousTotalHeight: 500, previousScrollHeight: 500, previousClientHeight: 100 }, unread: false };
function scroller(height = 100) {
  const element = document.createElement('div');
  for (const [key, value] of Object.entries({ clientHeight: height, clientWidth: 100, scrollHeight: 500, scrollTop: 400 })) Object.defineProperty(element, key, { configurable: true, value });
  return element;
}
describe('pure owning anchor probe', () => {
  it('requires current command identity and owner terminal state', () => {
    expect(observeOwningAnchor(intent, terminal, state, scroll, null)).toEqual({ kind: 'target', messageId: 'target', disposition: 'applied' });
    expect(observeOwningAnchor(intent, terminal, { ...state, terminalCommandKeys: new Set() }, scroll, null)).toEqual({ kind: 'pending' });
  });
  it.each([
    { ...command, token: 5 },
    { ...command, requestToken: 1 },
    { ...command, view: { ...command.view, conversationId: 'other' } },
    { ...command, view: { ...command.view, generation: 4 } },
    { ...command, view: { ...command.view, transcriptGeneration: 4 } },
    { ...command, targetMessageId: 'new-target' },
  ])('rejects predecessor success for a newer token/source/view or target', next => {
    expect(observeOwningAnchor({ ...intent, command: next }, terminal, state, scroll, null)).toEqual({ kind: 'pending' });
  });
  it('rejects changed selected segment and active superseding intent', () => {
    expect(observeOwningAnchor({ ...intent, scope: '4:new-segment:2' }, terminal, state, scroll, null)).toEqual({ kind: 'pending' });
    expect(observeOwningAnchor(intent, terminal, { ...state, active: { command, key: transcriptPositioningCommandKey(command) } }, scroll, null)).toEqual({ kind: 'pending' });
  });
  it.each(['target_missing', 'superseded'] as const)('preserves %s rather than using previous success', result => {
    expect(observeOwningAnchor(intent, { ...terminal, result }, state, scroll, null)).toEqual({ kind: 'target', messageId: 'target', disposition: result });
  });
  it('requires measurable current native tail and following owner, without writing geometry', () => {
    const element = scroller();
    expect(observeOwningAnchor({ scope: intent.scope, command: null, view: command.view }, null, state, scroll, element)).toEqual({ kind: 'tail', settled: true });
    expect(element.scrollTop).toBe(400);
    expect(observeOwningAnchor({ scope: intent.scope, command: null, view: { ...command.view, transcriptGeneration: 9 } }, null, state, scroll, element)).toEqual({ kind: 'pending' });
    expect(observeOwningAnchor({ scope: intent.scope, command: null, view: command.view }, null, state, scroll, scroller(0))).toEqual({ kind: 'pending' });
    expect(observeOwningAnchor({ scope: intent.scope, command: null, view: command.view }, null, state, { ...scroll, follow: { kind: 'reading' } }, element)).toEqual({ kind: 'pending' });
    Object.defineProperty(element, 'scrollTop', { value: 300 });
    expect(observeOwningAnchor({ scope: intent.scope, command: null, view: command.view }, null, state, scroll, element)).toEqual({ kind: 'tail', settled: false });
  });
});
