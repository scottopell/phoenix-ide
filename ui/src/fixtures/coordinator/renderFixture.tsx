import { useEffect, useMemo, useState } from 'react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import type { Conversation, GlobalOpenWorkResponse, ImageData } from '../../api';
import { InputArea } from '../../components/InputArea';
import { MessageList } from '../../components/MessageList';
import { StateBar } from '../../components/StateBar';
import { useDocumentViewportOwnership } from '../../components/viewportRoutes';
import { ConversationContext } from '../../conversation/ConversationContext';
import { ConversationStore } from '../../conversation/ConversationStore';
import { DensityContext } from '../../hooks/useDensity';
import { CoordinatorPage } from '../../pages/CoordinatorPage';
import { getMessageListScenario, messageListFixtureData } from '../messageList';
import '../../index.css';
import type { CoordinatorScenario } from './types';

interface Props {
  scenario: CoordinatorScenario;
}

const coordinatorId = 'fixture-coordinator';

export function CoordinatorFixture({ scenario }: Props) {
  useDocumentViewportOwnership(true);

  useEffect(() => {
    document.documentElement.dataset['theme'] = 'dark';
    document.documentElement.dataset['coordinatorFixtureReady'] = scenario.id;
    return () => { delete document.documentElement.dataset['coordinatorFixtureReady']; };
  }, [scenario]);

  return (
    <MemoryRouter initialEntries={[`/global/${coordinatorId}`]}>
      <Routes>
        <Route
          path="/global/:slug"
          element={(
            <CoordinatorPage
              fixtureData={{
                coordinatorId,
                openWork: fixtureOpenWork,
                initialView: scenario.initialView,
                ...(scenario.fleetError ? { workError: 'projection unavailable' } : {}),
                conversation: <FixtureConversation working={scenario.working} />,
              }}
            />
          )}
        />
      </Routes>
    </MemoryRouter>
  );
}

function FixtureConversation({ working }: { working: boolean }) {
  const [draft, setDraft] = useState('');
  const [images, setImages] = useState<ImageData[]>([]);
  const store = useMemo(() => new ConversationStore(), []);
  const transcript = useMemo(
    () => messageListFixtureData(getMessageListScenario('compact-latest-expanded')),
    [],
  );
  const convState = working ? { type: 'llm_requesting', attempt: 1 } as const : { type: 'idle' } as const;
  const conversation: Conversation = {
    id: transcript.conversationId,
    slug: transcript.slug,
    model: 'claude-sonnet-5',
    cwd: '/work/phoenix',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    message_count: transcript.messages.length,
    state: convState,
    branch_name: null,
    base_branch: null,
    worktree_path: null,
    task_title: null,
    conv_mode_label: 'Explore',
    browser_session_active: false,
    terminal_uses_tmux: false,
    work_scope_key: 'global:',
  };

  return (
    <ConversationContext.Provider value={store}>
      <DensityContext.Provider value={{ density: 'compact', setDensity: () => {} }}>
        <div id="app">
          <div className="conversation-column">
            <MessageList
              messages={transcript.messages}
              pendingMessages={[]}
              convState={convState}
              onRetry={() => {}}
              onOpenFile={() => {}}
              conversationId={transcript.conversationId}
              slug={transcript.slug}
              transcriptPositioning={{ kind: 'idle', view: { conversationId: transcript.conversationId, generation: 1, transcriptGeneration: 1 } }}
            />
            <InputArea
              cwd={undefined}
              scopeKey={transcript.conversationId}
              convState={convState}
              images={images}
              setImages={setImages}
              isOffline={false}
              failedMessages={[]}
              convModeLabel="Explore"
              draft={draft}
              onDraftChange={setDraft}
              onSend={() => {}}
              onCancel={() => {}}
              onRetry={() => {}}
            />
            <StateBar
              conversation={conversation}
              convState={convState}
              connectionState="connected"
              connectionAttempt={0}
              nextRetryIn={null}
              contextWindowUsed={16_000}
              modelContextWindow={200_000}
              phaseStateUpdatedAt={null}
            />
          </div>
        </div>
      </DensityContext.Provider>
    </ConversationContext.Provider>
  );
}

const fixtureOpenWork: GlobalOpenWorkResponse = {
  generated_at: '2026-01-01T00:00:00Z',
  has_more: false,
  groups: [{
    project_id: 'phoenix',
    project_name: 'Phoenix',
    canonical_path: '/work/phoenix',
    items: [{
      id: 'mobile-first-coordinator',
      source: 'chain',
      title: 'Restore mobile Coordinator transcript',
      project_id: 'phoenix',
      current_conversation_id: 'a27dd240-2fb9-426e-835c-3cc48cd84c24',
      current_conversation_slug: 'mobile-first-coordinator',
      root_conversation_id: '86f2ce14-durable-coordinator',
      root_conversation_slug: 'durable-coordinator',
      updated_at: '2026-01-01T12:00:00Z',
      mode: 'WORK',
      state: 'working',
      task_id: '44006',
      task_title: 'Make Coordinator mobile-first',
      task_status: 'in-progress',
      branch_name: 'task-44006-mobile-first-coordinator',
      base_branch: 'main',
      worktree_path: '/phoenix/worktrees/mobile-first-coordinator',
      member_count: 2,
      signals: ['needs action', 'task open'],
      href: '/c/mobile-first-coordinator',
      reference: '@work:mobile-first-coordinator',
    }],
  }],
};
