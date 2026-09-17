import { useEffect, useState } from 'react';
import { api, QuestionMutationError, type Conversation } from '../../api';
import { QuestionPanel } from '../../components/QuestionPanel';
import { FocusScopeProvider } from '../../hooks/useFocusScope';
import type { AskUserQuestionScenario } from './scenarios';
import '../../index.css';

export function AskUserQuestionFixture({ scenario }: { scenario: AskUserQuestionScenario }) {
  const [ready, setReady] = useState(false);
  const [result, setResult] = useState('No response sent');
  const [toast, setToast] = useState('');
  const [finished, setFinished] = useState(false);
  useEffect(() => {
    const theme = document.documentElement.dataset['theme'];
    document.documentElement.dataset['theme'] = 'light';
    const respond = api.respondToQuestion;
    const dismiss = api.dismissQuestion;
    const getConversation = api.getConversationStatus;
    api.getConversationStatus = async () => ({ conversation: { id: 'fixture-auq', state: { type: 'idle' } } as Conversation, agent_working: false, presentation_mode: 'chat' });
    api.respondToQuestion = async (_id, requestId, answers, annotations) => {
      if (scenario.fail) throw new QuestionMutationError('Fixture: response rejected. Please retry.', 'question_request_invalid');
      setResult(JSON.stringify({ requestId, answers, annotations }, null, 2));
      return { success: true };
    };
    api.dismissQuestion = async () => {
      setResult('Dismissed without an answer');
      return { success: true };
    };
    setReady(true);
    return () => {
      api.respondToQuestion = respond;
      api.dismissQuestion = dismiss;
      api.getConversationStatus = getConversation;
      if (theme === undefined) delete document.documentElement.dataset['theme'];
      else document.documentElement.dataset['theme'] = theme;
    };
  }, [scenario]);
  return (
    <FocusScopeProvider>
      <main className="conversation-column" data-ask-user-question-fixture={ready ? scenario.id : undefined} style={{ height: '100dvh', display: 'flex', flexDirection: 'column', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}>
        <section className="question-fixture-transcript" style={{ flex: 1, minHeight: 0, overflow: 'auto', padding: 16 }}>
          <h2>AskUserQuestion validation: {scenario.id}</h2>
          <p>Fixture transcript. Submitted responses appear here.</p>
          <pre aria-label="Captured response">{result}</pre>
          <p role="status">{toast}</p>
        </section>
        {ready && !finished && <QuestionPanel questions={scenario.questions} conversationId="fixture-auq" requestId="fixture-question" onResolved={() => setFinished(true)} showToast={setToast} readOnly={scenario.readOnly ?? false} />}
        <footer style={{ padding: 12 }}>Fixture conversation · {finished ? 'Response handled' : 'Awaiting your reply'}</footer>
      </main>
    </FocusScopeProvider>
  );
}
