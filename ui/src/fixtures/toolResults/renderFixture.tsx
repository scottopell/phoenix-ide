import { useEffect, useMemo } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { AgentMessage } from '../../components/MessageComponents';
import { ViewerSlotProvider } from '../../contexts/ViewerSlotContext';
import { ForkProposalsProvider } from '../../contexts/ForkProposalsContext';
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

export function ToolResultsFixture({ scenario }: Props) {
  const data = toolResultsFixtureData(scenario);
  const store = useMemo(() => new ConversationStore(), []);
  const restoreApi = useMemo(() => installToolResultsFixtureApi(), []);
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

  useEffect(() => restoreApi, [restoreApi]);

  document.documentElement.dataset['theme'] = data.theme;

  return (
    <ConversationContext.Provider value={store}>
      <DensityContext.Provider value={{ density: data.density, setDensity: () => {} }}>
        <MemoryRouter initialEntries={[`/c/${data.slug}`]}>
          <ForkProposalsProvider conversationId={data.conversationId}>
            <ViewerSlotProvider scopeKey={data.workScopeKey} browserSessionActive={false}>
              <main
              className="fixture-page"
              data-tool-results-fixture={scenario.id}
              data-tool-results-fixture-ready={scenario.id}
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
          </ForkProposalsProvider>
        </MemoryRouter>
      </DensityContext.Provider>
    </ConversationContext.Provider>
  );
}
