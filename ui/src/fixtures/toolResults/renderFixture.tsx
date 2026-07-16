import { useEffect, useMemo, useState } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { AgentMessage } from '../../components/MessageComponents';
import { ViewerSlotProvider } from '../../contexts/ViewerSlotContext';
import { ForkProposalsProvider, useForkProposals } from '../../contexts/ForkProposalsContext';
import { ConversationContext } from '../../conversation/ConversationContext';
import { ConversationStore } from '../../conversation/ConversationStore';
import { DensityContext } from '../../hooks/useDensity';
import { buildHistoricalUnits } from '../../conversation/renderUnits';
import '../../index.css';
import './toolResultsFixture.css';
import { toolResultsFixtureData } from './scenarios';
import { installToolResultsFixtureApi } from './mockApi';
import type { ToolResultsScenario } from './types';

interface Props {
  scenario: ToolResultsScenario;
}

function ToolResultsFixtureContent({ scenario }: Props) {
  const forkProposals = useForkProposals();
  const proposalReady = scenario.family !== 'shell'
    || (forkProposals?.loaded === true && forkProposals.getProposal('fixture-fork-proposal') !== undefined);

  return <ToolResultsFixtureBody scenario={scenario} ready={proposalReady} />;
}

function ToolResultsFixtureBody({ scenario, ready }: Props & { ready: boolean }) {
  const data = toolResultsFixtureData(scenario);
  const store = useMemo(() => new ConversationStore(), []);
  const activeToolUseId = data.convState.type === 'tool_executing'
    ? data.convState.current_tool.id
    : undefined;
  const renderedMessages = useMemo(() => data.messages.map((message) => {
    if (message.message_id !== 'agent-13') return message;
    return {
      ...message,
      display_data: { tool_starts: { 'shell-pending': Date.now() - 4_000 } },
    };
  }), [data.messages]);
  const agentTurns = useMemo(
    () => buildHistoricalUnits({ messages: renderedMessages, pendingMessages: [] }).historicalUnits
      .filter((unit) => unit.kind === 'agent_turn'),
    [renderedMessages],
  );

  document.documentElement.dataset['theme'] = data.theme;

  return (
    <ConversationContext.Provider value={store}>
      <DensityContext.Provider value={{ density: data.density, setDensity: () => {} }}>
        <MemoryRouter initialEntries={[`/c/${data.slug}`]}>
          <ViewerSlotProvider scopeKey={data.workScopeKey} browserSessionActive={false}>
              <main
              className="fixture-page"
              data-tool-results-fixture={scenario.id}
              {...(ready ? { 'data-tool-results-fixture-ready': scenario.id } : {})}
            >
              <div className="fixture-toolbar">
                <strong>Tool results fixture</strong>
                <span>scenario={scenario.id}</span>
                <span>family={scenario.family}</span>
                <span>density={data.density}</span>
              </div>
              <div className="fixture-message-list-stage tool-results-fixture-stage">
                <div className="message-list-fixture-shell tool-results-fixture-shell">
                  {agentTurns.map((turn) => (
                    <AgentMessage
                      key={turn.key}
                      message={turn.agent}
                      toolResults={turn.toolResultsByUseId}
                      onOpenFile={() => {}}
                      filePathRootDir={data.filePathRootDir}
                      workScopeKey={data.workScopeKey}
                      activeToolUseId={activeToolUseId}
                      isFirstInTurn={turn.isFirstInTurn}
                      isLatestAgentMessage={turn === agentTurns.at(-1)}
                      forceExpandedText={turn === agentTurns.at(-1)}
                    />
                  ))}
                </div>
              </div>
              </main>
          </ViewerSlotProvider>
        </MemoryRouter>
      </DensityContext.Provider>
    </ConversationContext.Provider>
  );
}

export function ToolResultsFixture({ scenario }: Props) {
  const [mockInstalled, setMockInstalled] = useState(false);
  const data = toolResultsFixtureData(scenario);

  useEffect(() => {
    const restoreApi = installToolResultsFixtureApi();
    setMockInstalled(true);
    return restoreApi;
  }, []);

  if (!mockInstalled) return null;

  return (
    <ForkProposalsProvider ownerGeneration={1} conversationId={data.conversationId}>
      <ToolResultsFixtureContent scenario={scenario} />
    </ForkProposalsProvider>
  );
}
