# Align Codex-auth request prefixes with upstream Responses Lite

Upstream codex-rs GPT-5.6 requests use Responses Lite: tools are serialized as a leading developer `additional_tools` input item, base instructions as a following developer message, top-level `instructions` is empty, top-level `tools` is omitted, parallel tool calls are disabled, and reasoning context is `all_turns`. Phoenix currently sends platform-style top-level instructions/tools to the ChatGPT Codex backend. Implement a backend-specific, structurally typed Responses Lite translator and compatibility header based on a pinned upstream request contract. Preserve direct-platform request behavior.

Acceptance criteria:
- [ ] Codex-auth GPT-5.6 requests match upstream Responses Lite prefix semantics and headers.
- [ ] Direct OpenAI platform requests retain platform wire shape and explicit caching support.
- [ ] Golden wire tests cover tools, instructions, messages, tool loops, and model gating.
- [ ] Prefix-stability tests prove unchanged configuration serializes byte-identically across turns.
- [ ] Capability differences are represented by backend-specific types, not conditionals that permit invalid combinations.
- [ ] Specs document the Codex Responses Lite contract and capability gaps.
- [ ] Live Codex-auth measurements compare cache-read ratios before and after the change.
- [ ] `./dev.py check` passes.
