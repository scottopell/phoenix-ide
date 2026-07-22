export interface ConversationReturnOrigin {
  kind: 'coordinator';
  href: string;
}

export interface ConversationLinkState {
  conversationReturnOrigin: ConversationReturnOrigin;
}

export function coordinatorConversationLinkState(
  currentPath: string,
  destination: string,
): ConversationLinkState | undefined {
  if (!/^\/global(?:\/|$)/.test(currentPath) || !/^\/c\//.test(destination)) return undefined;
  return { conversationReturnOrigin: { kind: 'coordinator', href: currentPath } };
}
