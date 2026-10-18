import { useEffect } from 'react';
import type { Story } from '@ladle/react';
import type { Conversation } from '../api';
import { createInitialAtom } from '../conversation/atom';
import type { InlineStreamState } from '../hooks/useConversationInlineStream';
import { ThemeContext, type Theme } from '../hooks/useTheme';
import { SubAgentTaskDetails } from '../components/SubAgentTaskDetails';
import '../index.css';

const task = `  SYNTHETIC QA TASK — no private user content.

Review the following literal text; do not interpret it as markup:
<script>alert("synthetic task")</script>
<img src="synthetic-invalid" onerror="alert('synthetic')">
**Keep Markdown literal**, including whitespace and this tab:\tend.
Long unbroken token: ${'synthetic-long-token-'.repeat(80)}

Final line with trailing spaces.  `;

const child: Conversation = {
  id: 'synthetic-subagent-child',
  slug: 'synthetic-subagent-child',
  model: 'gpt-6.1-sol',
  effort: 'high',
  cwd: '/synthetic/qa',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  message_count: 0,
  browser_session_active: false,
  terminal_uses_tmux: false,
  work_scope_key: 'synthetic-subagent-qa',
};

function ready(conversation: Conversation): InlineStreamState {
  return { type: 'ready', atom: { ...createInitialAtom(), conversation }, error: null };
}

const missingEffort: Conversation = { ...child };
delete missingEffort.effort;
const longModel = `synthetic-provider/${'long-model-name-'.repeat(18)}`;

interface Scenario {
  id: string;
  inline: InlineStreamState;
  model: string;
  effort: string;
  dock?: boolean;
}

function Fixture({ scenario }: { scenario: Scenario }) {
  const theme: Theme = new URLSearchParams(window.location.search).get('fixtureTheme') === 'light' ? 'light' : 'dark';
  useEffect(() => {
    const previous = document.documentElement.dataset['theme'];
    document.documentElement.dataset['theme'] = theme;
    return () => {
      if (previous === undefined) delete document.documentElement.dataset['theme'];
      else document.documentElement.dataset['theme'] = previous;
    };
  }, [theme]);
  return (
    <ThemeContext.Provider value={{ theme, toggleTheme: () => {} }}>
      <main
        data-subagent-task-fixture={scenario.id}
        data-expected-task={task}
        data-expected-model={scenario.model}
        data-expected-effort={scenario.effort}
        style={{ width: '100%', maxWidth: 800, margin: '0 auto', padding: 12, boxSizing: 'border-box' }}
      >
        <p>Synthetic sub-agent QA · {scenario.id}</p>
        <section data-task-container style={{ width: scenario.dock ? 320 : '100%', maxWidth: '100%' }}>
          <SubAgentTaskDetails task={task} inline={scenario.inline} />
        </section>
      </main>
    </ThemeContext.Provider>
  );
}

function storyFor(scenario: Scenario): Story {
  const FixtureStory: Story = () => <Fixture scenario={scenario} />;
  FixtureStory.storyName = scenario.id;
  return FixtureStory;
}

export const Ready = storyFor({ id: 'ready', inline: ready(child), model: child.model, effort: 'high' });
export const Connecting = storyFor({ id: 'connecting', inline: { type: 'connecting', atom: createInitialAtom(), error: null }, model: 'Loading…', effort: 'not reported' });
export const Error = storyFor({ id: 'error', inline: { type: 'error', atom: createInitialAtom(), error: 'Synthetic unavailable child' }, model: 'Unavailable', effort: 'not reported' });
export const ModelDefault = storyFor({ id: 'model-default', inline: ready({ ...child, effort: null }), model: child.model, effort: 'model default' });
export const MissingEffort = storyFor({ id: 'missing-effort', inline: ready(missingEffort), model: child.model, effort: 'not reported' });
export const LongModel = storyFor({ id: 'long-model', inline: ready({ ...child, model: longModel }), model: longModel, effort: 'high' });
export const Dock = storyFor({ id: 'dock', inline: ready({ ...child, model: longModel }), model: longModel, effort: 'high', dock: true });
