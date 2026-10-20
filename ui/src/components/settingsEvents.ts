export const OPEN_FEDERATION_CONNECTIONS_EVENT = 'phoenix:open-federation-connections';

export function openFederationConnections() {
  window.dispatchEvent(new Event(OPEN_FEDERATION_CONNECTIONS_EVENT));
}
