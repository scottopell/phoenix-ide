# Named workers and execution tiers

Phoenix reads `$XDG_CONFIG_HOME/phoenix-ide/config.toml`, or
`$HOME/.config/phoenix-ide/config.toml` when `XDG_CONFIG_HOME` is unset.
The file is loaded when a parent runtime is created. It is not watched for edits.

```toml
version = 1

[agents.reviewer]
description = "Review a bounded change for correctness."
instructions = "Find concrete defects. Explain the trigger and consequence."
execution = [
  { model = "gpt-5.6-sol", connection = "codex", reasoning_effort = "high" },
]

[tiers.fast]
execution = [
  { model = "gpt-5.6-luna", connection = "codex", reasoning_effort = "low" },
]

[tiers.capable]
execution = [
  { model = "gpt-5.6-sol", connection = "codex", reasoning_effort = "high" },
]
```

Use model identifiers from your installation's catalog. Connection names identify
configured backend slots: `codex`, `anthropic`, `openai_responses`,
`openai_chat_completions`, or `mock`. The file selects connections; it does not
configure credentials or create provider connections.

Candidates are tried in order. An unavailable route is skipped. A reached usable
route with incompatible effort produces a configuration diagnostic rather than
falling back to hide the mistake. Omitting `reasoning_effort` uses that model's
native default. Workers and tiers with no usable choice are not offered to the LLM.

## What the LLM can request

Within each `spawn_agents` task:

- `agent_type: "reviewer"` uses the worker's instructions and default execution.
- `execution: {"type": "tier", "name": "fast"}` selects the configured tier.
- `execution: {"type": "model", "model": "gpt-5.6-sol", "connection": "codex", "reasoning_effort": "high"}` selects an exact route.

A worker can be combined with one execution selector. The selector replaces its
execution preferences while keeping its instructions. Without a worker's candidate
list or an explicit selector, the child inherits the parent's model, connection,
and effort. An exact model choice never silently falls back.

Workers do not contain `mode`, `tools`, or permissions. The spawn request asks
for authority separately and Phoenix enforces the parent's workspace limits.

## Moving existing agents

Copy the desired instructions and description into an `[agents.NAME]` entry.
Replace a single default model with an ordered list of model/connection/effort
candidates. Omit old mode, tools, and skill-link fields; reusable expertise belongs
in Skills. Phoenix does not scan `.claude/agents` or `.agents/agents`, merge those
files into this catalog, or modify them automatically.
