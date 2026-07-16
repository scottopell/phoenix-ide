# Honor configured OAuth scopes for HTTP MCP servers

Phoenix consumes Claude Code-compatible MCP configuration, but its HTTP OAuth parser currently discards `oauth.scopes`. The discarded value cannot be represented by `HttpAuth::OAuth(Option<PreconfiguredClient>)`; `PreconfiguredClient` carries only `client_id` and `callback_port`, and `begin_oauth_flow` can therefore request scopes only from a 401 challenge, Protected Resource Metadata, or a later step-up. Slack now rejects Phoenix's resulting authorization request with `no_bot_scopes_requested` when that discovery path yields no scopes.

## Plan

1. Extend the normative MCP requirements and Allium behavior so an OAuth config may supply an explicit whitespace-delimited initial scope set. Define scope construction without parallel authorities: configured scopes participate in the requested set, challenge-required scopes and persisted step-up scopes are unioned into it, and discovered Protected Resource Metadata remains the fallback when no explicit/challenged scope exists.
2. Replace the lossy `HttpAuth::OAuth(Option<PreconfiguredClient>)` representation with a typed OAuth configuration that can represent both optional preconfigured client identity and requested scopes. Preserve `clientId` and `callbackPort` behavior, support scopes with or without a preconfigured client, normalize whitespace, deduplicate values, and reject or visibly log malformed values rather than silently dropping them.
3. Thread the typed configured scopes through `begin_oauth_flow` and authorization URL construction. Ensure a Slack-compatible config emits a non-empty `scope=` parameter while preserving the existing prior-grant/`insufficient_scope` union behavior.
4. Add focused tests for config classification, equality/reload behavior, scope precedence and unioning, and generated authorization URLs. Include regression coverage using the Slack-shaped config (`clientId`, callback port 3118, and configured scopes).
5. Update `specs/mcp/executive.md`, run the spec pre-flight checks from `specs/AUTHORING.md`, and run the targeted MCP tests plus `./dev.py check`.

## Acceptance criteria

- Phoenix reads Claude Code's `oauth.scopes` string instead of discarding it.
- An OAuth config containing scopes but no `clientId` retains those scopes while using dynamic client registration.
- Slack authorization URLs contain the configured user scopes and no longer fail with `no_bot_scopes_requested`.
- Challenge-required and persisted step-up scopes are not lost when configured scopes are present.
- Existing OAuth configurations with `oauth: true`, `clientId`, and/or `callbackPort` continue to work.
