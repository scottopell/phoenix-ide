import { getMobileConversationListScenario, MobileConversationListFixture } from '../fixtures/mobileConversationList';

export function MobileConversationListFixturePage() {
  const params = new URLSearchParams(window.location.search);
  const scenario = getMobileConversationListScenario(params.get('scenario') ?? params.get('id'));
  return <MobileConversationListFixture scenario={scenario} />;
}
