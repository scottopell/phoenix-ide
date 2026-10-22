import { ViewJourneyReporter } from '../src/hooks/conversationViewTelemetry';
import { ViewJourneyLifecycle, defaultViewIdentity } from '../src/hooks/conversationViewLifecycle';

// Synthetic adapter, not the React hook/router or a Phoenix conversation page.
const unsampled = location.pathname.endsWith('/unsampled');
const reporter = new ViewJourneyReporter({
  random: () => unsampled ? 0.9 : 0.1,
  send: (input, init) => fetch(input, init),
});
const lifecycle = new ViewJourneyLifecycle(defaultViewIdentity('ordinary', 'navigation'), reporter);
lifecycle.attach(document.body);
const requestId = crypto.randomUUID();
const observation = lifecycle.requestStarted(requestId);
void fetch(`/api/product-conversations/11111111-1111-4111-8111-111111111111?open_id=${requestId}`)
  .then(() => { /* The sampled finite request is deliberately held until pagehide. */ })
  .catch(() => { /* Navigation cancellation is not a failed-view signal in this adapter. */ });
window.addEventListener('pagehide', () => {
  lifecycle.visibility(false);
  lifecycle.finish('incomplete');
  // Explicit deferred-response trigger; the collector also awaits the actual POST body.
  void fetch('/probe/pagehide', { method: 'POST', keepalive: true }).catch(() => {});
});
if (unsampled && observation === undefined) {
  document.body.innerHTML = '<div id="messages"><div class="empty-state">Synthetic unsampled control</div></div><nav id="conversation-nav-trail"><button class="conversation-nav-item" role="button">Synthetic control</button></nav>';
}

// No report/mark fabrication, factory substitution, fetch shim or global random override.
void lifecycle.openId;
void reporter;
