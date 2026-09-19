# Avoid advertising models unavailable to the configured authentication provider

During live SVG publication verification, GET /api/models advertised gpt-5.4-mini under OpenAI while the selected provider used Codex ChatGPT-account authentication. Creating a Direct conversation with that advertised model failed with provider HTTP 400: "The 'gpt-5.4-mini' model is not supported when using Codex with a ChatGPT account."

Reproduction: use a ChatGPT-backed Codex connection, list models, select gpt-5.4-mini, and send a simple first message. gpt-5.5 worked with the same configuration. Investigate capability-aware model advertisement/selection without hiding API-key-supported choices globally. Add coverage that every recommended selectable model is supported by its selected connection or explicitly labeled unavailable before conversation creation.
