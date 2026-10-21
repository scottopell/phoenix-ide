export const OPEN_FEDERATION_CONNECTIONS_EVENT = 'phoenix:open-federation-connections';
export const SETTINGS_QUERY_KEY = 'settings';
export const FEDERATION_CONNECTIONS_SETTINGS_VALUE = 'connections';
export const FEDERATION_CONNECTIONS_SETTINGS_HREF = `/?${SETTINGS_QUERY_KEY}=${FEDERATION_CONNECTIONS_SETTINGS_VALUE}`;

export function requestOpenFederationConnections(): boolean {
  return !window.dispatchEvent(new Event(OPEN_FEDERATION_CONNECTIONS_EVENT, { cancelable: true }));
}
