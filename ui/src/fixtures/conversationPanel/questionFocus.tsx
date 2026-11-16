import { useEffect, useState } from 'react';
import type { ConversationState } from '../../api';
import { QuestionPanel } from '../../components/QuestionPanel';
import { FocusScopeProvider } from '../../hooks/useFocusScope';
import { conversationReducer, createInitialAtom, type InitPayload } from '../../conversation/atom';
import { conversationPanelFixtureData } from './scenarios';
import '../../index.css';

const phase: Extract<ConversationState, { type: 'awaiting_user_response' }> = {
  type: 'awaiting_user_response', request_id: 'focus-request',
  questions: ['Q1?', 'Q2?'].map(question => ({ question, header: 'Choice', multiSelect: false,
    options: [{ label: 'First', description: 'First option' }, { label: 'Second', description: 'Second option' }],
  })),
};
function payload(sequence = 1): InitPayload {
  return { opening: null, conversation: { ...conversationPanelFixtureData.conversations[0]!, id: 'focus-fixture', state: structuredClone(phase) },
    messages: [], steeringMessages: [], phase: structuredClone(phase), contextWindow: { used: 0 },
    transcriptGeneration: 1, streamIncarnation: 'focus-fixture', lastAppliedEventSeq: sequence,
    pendingAnchorSequenceId: sequence, pendingEvents: [], pendingTruncated: false, transcriptCoverage: 'complete' };
}

export function QuestionFocusFixture() {
  const [atom, setAtom] = useState(() => conversationReducer(
    conversationReducer(createInitialAtom(), { type: 'connection_opened', epoch: 1 }),
    { type: 'sse_init', epoch: 1, payload: payload() },
  ));
  const [shown, setShown] = useState(true);
  const [accepted, setAccepted] = useState('mount');
  useEffect(() => {
    const refresh = (event: Event) => {
      const kind = (event as CustomEvent<string>).detail;
      setAtom(old => {
        const next = conversationReducer(old, kind === 'reconnect'
          ? { type: 'sse_init', epoch: 1, payload: payload(old.lastAppliedEventSeq) }
          : { type: 'sse_state_change', epoch: 1, sequenceId: old.lastAppliedEventSeq + 1, phase: structuredClone(phase), stateUpdatedAt: 0 });
        if (next.phase === old.phase || JSON.stringify(next.phase) !== JSON.stringify(old.phase)) {
          throw new Error('Question focus fixture refresh was rejected or changed question value');
        }
        return next;
      });
      setAccepted(kind);
    };
    const show = (event: Event) => setShown((event as CustomEvent<boolean>).detail);
    window.addEventListener('question-focus-refresh', refresh);
    window.addEventListener('question-focus-show', show);
    return () => {
      window.removeEventListener('question-focus-refresh', refresh);
      window.removeEventListener('question-focus-show', show);
    };
  }, []);
  if (atom.phase.type !== 'awaiting_user_response' || !atom.phase.request_id) throw new Error('Expected question phase with request identity');
  return <FocusScopeProvider>
    <section data-conversation-panel-fixture-ready="question-focus" data-question-refresh={accepted}>
      <input id="focus-outside" aria-label="Outside focus target" />
      <div id="focus-ancestor" style={{ height: 400, overflow: 'auto' }}>
        <div style={{ height: 600 }} />
        {shown && <QuestionPanel key={atom.phase.request_id} questions={atom.phase.questions}
          conversationId="focus-fixture" requestId={atom.phase.request_id}
          showToast={() => {}} onAnswered={() => {}} onDismissed={() => {}} />}
        <div style={{ height: 600 }} />
      </div>
    </section>
  </FocusScopeProvider>;
}
