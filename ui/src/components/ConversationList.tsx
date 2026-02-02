import { useNavigate } from 'react-router-dom';
import type { Conversation } from '../api';
import { formatRelativeTime } from '../utils';

interface ConversationListProps {
  conversations: Conversation[];
  onNewConversation: () => void;
}

export function ConversationList({ conversations, onNewConversation }: ConversationListProps) {
  const navigate = useNavigate();

  const handleClick = (conv: Conversation) => {
    navigate(`/c/${conv.slug}`);
  };

  return (
    <section id="conversation-list" className="view active">
      <div className="view-header">
        <h2>Conversations</h2>
        <button id="new-conv-btn" className="btn-primary" onClick={onNewConversation}>
          + New
        </button>
      </div>
      <ul id="conv-list">
        {conversations.length === 0 ? (
          <li className="empty-state">
            <div className="empty-state-icon">💬</div>
            <p>No conversations yet</p>
          </li>
        ) : (
          conversations.map((conv) => (
            <li
              key={conv.id}
              className="conv-item"
              data-id={conv.id}
              onClick={() => handleClick(conv)}
            >
              <div className="conv-item-slug">{conv.slug}</div>
              <div className="conv-item-meta">
                <span>{formatRelativeTime(conv.updated_at)}</span>
                <span className="conv-item-cwd">{conv.cwd}</span>
              </div>
            </li>
          ))
        )}
      </ul>
    </section>
  );
}
