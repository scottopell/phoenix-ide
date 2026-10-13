import { act, renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';
import { expect, it } from 'vitest';
import { ConversationStore } from './ConversationStore';
import { ConversationContext } from './ConversationContext';
import { useConversationTranscriptView, useConversationView } from './useConversationAtom';
import type { Conversation } from '../api';

it('publishes opening-only changes to the direct transcript selector without re-rendering the controls projection', () => {
  const store = new ConversationStore();
  store.dispatch('successor', { type: 'set_initial_data', conversationId: 'successor',
    conversation: { id: 'successor', slug: 'successor' } as Conversation, messages: [], phase: { type: 'idle' }, contextWindow: { used: 0 } });
  const wrapper = ({ children }: { children: ReactNode }) => <ConversationContext.Provider value={store}>{children}</ConversationContext.Provider>;
  let transcriptRenders = 0;
  let controlsRenders = 0;
  const transcript = renderHook(() => { transcriptRenders++; return useConversationTranscriptView('successor'); }, { wrapper });
  renderHook(() => { controlsRenders++; return useConversationView('successor'); }, { wrapper });
  const baseline = { transcript: transcriptRenders, controls: controlsRenders };
  const opening = { status: 'accepted' as const, edge: { product_conversation_id: 'product', predecessor_transcript_row_id: 'predecessor', successor_transcript_row_id: 'successor', continuation_message_id: 'boundary' }, accepted_message_id: 'opening', opening_authority: 'generated_predecessor_context' as const, presentation: 'boundary' as const };
  act(() => store.dispatch('successor', { type: 'sse_message_updated', sequenceId: 1, messageId: 'opening', opening }));
  expect(transcript.result.current?.opening).toEqual(opening);
  expect(transcriptRenders).toBeGreaterThan(baseline.transcript);
  expect(controlsRenders).toBe(baseline.controls);
});
