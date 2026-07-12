import { MemoryRouter } from 'react-router-dom';
import { AgentMessage } from '../../components/MessageComponents';
import { CommissionReviewApproval } from '../../components/CommissionReviewApproval';
import { ViewerSlotProvider } from '../../contexts/ViewerSlotContext';
import { ThemeContext } from '../../hooks/useTheme';
import '../../index.css';
import { commissionReviewFixtureData } from './scenarios';
import type { CommissionReviewScenario } from './types';

interface Props {
  scenario: CommissionReviewScenario;
}

const noop = () => {};

export function CommissionReviewFixture({ scenario }: Props) {
  const data = commissionReviewFixtureData(scenario);
  document.documentElement.dataset['theme'] = data.theme;

  return (
    <ThemeContext.Provider value={{ theme: data.theme, toggleTheme: noop }}>
      <MemoryRouter>
        <ViewerSlotProvider browserSessionActive={false} scopeKey="fixture-commission-review">
          <main
            className="fixture-page"
            data-commission-review-fixture={scenario.id}
            data-commission-review-fixture-ready={scenario.id}
          >
            <div className="fixture-toolbar">
              <strong>Commission review fixture</strong>
              <span>{scenario.title}</span>
              <span>theme={data.theme}</span>
            </div>
            <p className="fixture-description">{scenario.description}</p>
            <div className="commission-review-fixture-stage">
              {scenario.kind.startsWith('approval-') ? (
                <CommissionReviewApproval
                  brief={data.approval.brief}
                  focus={data.approval.focus}
                  scope={data.approval.scope}
                  onApprove={() => {}}
                  onReject={() => {}}
                />
              ) : (
                <div className="message-list-fixture-shell tool-results-fixture-shell">
                  <AgentMessage
                    message={data.inline.message}
                    toolResults={data.inline.toolResults}
                    onOpenFile={() => {}}
                    filePathRootDir="/Users/example/src/phoenix-ide"
                    workScopeKey="fixture-commission-review"
                    activeToolUseId={data.inline.activeToolUseId}
                    isFirstInTurn
                    isLatestAgentMessage
                    forceExpandedText
                  />
                </div>
              )}
            </div>
          </main>
        </ViewerSlotProvider>
      </MemoryRouter>
    </ThemeContext.Provider>
  );
}
