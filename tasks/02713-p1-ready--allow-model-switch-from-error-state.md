Users cannot switch models when a conversation is in an error state.

Observed while debugging `/c/terminal-default-collapsed-state`: Anthropic/Opus returned an overload error, and the recommended workaround was to switch to another model. However, the UI/runtime prevents changing the selected model once the conversation has entered Error, so the user is stuck unless they start a new conversation or recover through another indirect path.

Expected behavior:
- A conversation in Error state should still allow model selection changes.
- After switching models, the user should be able to retry/continue the conversation with the new model.
- The model switch should be persisted and reflected consistently in the conversation header/settings.

Acceptance notes:
- Verify the model selector is enabled or an equivalent recovery action is available in Error state.
- Verify retry/continue uses the newly selected model, not the failed model.
- Preserve guardrails for states where changing models is genuinely unsafe, such as active LLM requests or tool execution.
