import { useLocation, useNavigate } from 'react-router-dom';
import type { ConversationLinkState } from './conversationReturnOrigin';

export function ConversationReturnBreadcrumb() {
  const location = useLocation();
  const navigate = useNavigate();
  const candidate = (location.state as Partial<ConversationLinkState> | null)?.conversationReturnOrigin;
  if (candidate?.kind !== 'coordinator' || !/^\/global(?:\/|$)/.test(candidate.href)) return null;

  return (
    <div className="conversation-seed-breadcrumb">
      <a href={candidate.href} onClick={(event) => {
        event.preventDefault();
        navigate(candidate.href);
      }}>
        {'\u2190'} from: Coordinator
      </a>
    </div>
  );
}
