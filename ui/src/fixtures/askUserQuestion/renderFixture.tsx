import { useEffect, useState } from 'react';
import { api } from '../../api';
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
    api.respondToQuestion = async (_id, answers, annotations) => {
      if (scenario.fail) throw new Error('Fixture: response failed. Please retry.');
      setResult(JSON.stringify({ answers, annotations }, null, 2));
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
      if (theme === undefined) delete document.documentElement.dataset['theme'];
      else document.documentElement.dataset['theme'] = theme;
    };
  }, [scenario]);
  return (
    <FocusScopeProvider>
      <main data-ask-user-question-fixture={ready ? scenario.id : undefined} style={{ height: '100dvh', display: 'flex', flexDirection: 'column', background: 'var(--bg-primary)', color: 'var(--text-primary)' }}>
        <section style={{ flex: 1, minHeight: 0, overflow: 'auto', padding: 16 }}>
          <h2>AskUserQuestion validation: {scenario.id}</h2>
          <p>Fixture transcript. Submitted responses appear here.</p>
          <pre aria-label="Captured response">{result}</pre>
          <p role="status">{toast}</p>
        </section>
        {ready && !finished && <QuestionPanel questions={scenario.questions} conversationId="fixture-auq" showToast={setToast} onAnswered={() => setFinished(true)} onDismissed={() => setFinished(true)} readOnly={scenario.readOnly ?? false} />}
        <footer style={{ padding: 12 }}>Fixture conversation · {finished ? 'Response handled' : 'Awaiting your reply'}</footer>
      </main>
    </FocusScopeProvider>
  );
}
