import { useEffect } from 'react';
import { TaskApprovalReader } from '../../components/TaskApprovalReader';
import '../../index.css';
import { taskApprovalFixturePlan } from './scenarios';
import type { TaskApprovalScenario } from './types';

interface Props {
  scenario: TaskApprovalScenario;
}

export function TaskApprovalFixture({ scenario }: Props) {
  useEffect(() => {
    delete document.documentElement.dataset['taskApprovalFixtureReady'];
    document.documentElement.dataset['theme'] = scenario.theme;
    const timer = window.setTimeout(() => {
      document.documentElement.dataset['taskApprovalFixtureReady'] = scenario.id;
    }, 50);
    return () => {
      window.clearTimeout(timer);
      delete document.documentElement.dataset['taskApprovalFixtureReady'];
    };
  }, [scenario]);

  return (
    <TaskApprovalReader
      title="Augment QC fixture set with canonical archive cases"
      priority="p3"
      plan={taskApprovalFixturePlan}
      contextWindowUsed={176_000}
      modelContextWindow={200_000}
      onApprove={() => {}}
      onReject={() => {}}
      onSendFeedback={() => {}}
    />
  );
}
