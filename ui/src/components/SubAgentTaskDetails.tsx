import type { InlineStreamState } from '../hooks/useConversationInlineStream';
import { CopyButton } from './CopyButton';
import './SubAgentTaskDetails.css';

interface Props {
  task: string;
  inline: InlineStreamState;
}

export function SubAgentTaskDetails({ task, inline }: Props) {
  const conversation = inline.atom.conversation;
  const model = conversation?.model || (
    inline.type === 'idle' || inline.type === 'connecting' ? 'Loading…' : 'Unavailable'
  );
  const effort = conversation?.effort;

  return (
    <div className="subagent-task-details">
      <div className="subagent-task-execution">
        <span>Model: {model}</span>
        <span>Effort: {effort === null ? 'model default' : effort ?? 'not reported'}</span>
      </div>
      <details className="subagent-delegated-task">
        <summary>Delegated task</summary>
        <div className="subagent-delegated-task-actions">
          <CopyButton text={task} title="Copy delegated task" />
        </div>
        <div className="subagent-delegated-task-text">{task}</div>
      </details>
    </div>
  );
}
