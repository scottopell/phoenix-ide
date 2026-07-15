# Drive E2E cancellation from positive turn evidence

`scenario_mid_stream_cancel` polls until conversation state is merely not idle/provisioning, then cancels and polls back to idle. This negative readiness predicate is timing-sensitive and does not bind cancellation to the exact message/turn being exercised.

Expose or consume a durable, identity-bound streaming/tool-start witness for the submitted message, send cancellation only after that positive witness, and require an identity-bound cancellation/finalization result. Keep an outer safety ceiling, but remove fixed-cadence sleeps as success synchronization. Include failure diagnostics with message identity and observed event/state history.
