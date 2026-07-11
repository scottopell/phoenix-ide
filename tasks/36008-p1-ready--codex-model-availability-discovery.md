# Discover account-specific Codex model availability

Phoenix registers all built-in GPT-5.6 models when Codex authentication is present, but live ChatGPT-backend probing found Sol and Terra available while Luna returned `Model not found gpt-5.6-luna`. Implement account-scoped model discovery or capability filtering using the upstream Codex model-list contract so the UI does not advertise models unavailable to the authenticated account.

Acceptance criteria:
- [ ] Codex-auth model availability comes from the account/backend capability response when available.
- [ ] Discovery failure has an explicit conservative fallback and does not silently claim unsupported models.
- [ ] Auth refresh/account switching invalidates cached availability.
- [ ] Tests cover partial GPT-5.6 availability, stale discovery, failure fallback, and direct API independence.
- [ ] Specs distinguish catalog membership from account-specific availability.
- [ ] `./dev.py check` passes.
