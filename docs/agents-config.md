# Named workers and execution tiers

Phoenix reads `$XDG_CONFIG_HOME/phoenix-ide/config.toml`, or
`$HOME/.config/phoenix-ide/config.toml` when `XDG_CONFIG_HOME` is unset.
Workers and tiers are loaded when a parent runtime is created. External models
are loaded when the Phoenix process starts. The file is not watched for edits.

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

## External model definitions

Add third-party models independently of the built-in catalog with top-level
`[[models]]` entries in this same file:

```toml
version = 1

[[models]]
id = "glm-5-3-flash"
backend = "openai_chat_completions"
family = "Z.ai"
description = "GLM 5.3 Flash via AI Gateway"
context_window = 1048576
max_output_tokens = 131072
recommended = false
supports_tool_search = false
```

Use the limits and request spelling verified for your provider route. `api_name`
is optional and defaults to `id`. The accepted backends are `anthropic`,
`openai_responses`, and `openai_chat_completions`. The fields match the external
JSON model format described in the [README](../README.md). Output limits must be
positive and smaller than the context window; unknown fields, duplicate external
IDs, or an invalid entry reject the entire TOML model section. Built-in IDs keep
their built-in definition. Model definitions still need the corresponding
endpoint and credentials; OpenAI-compatible external models do not use Codex login.

To move from the environment format after installing a binary with TOML model
support, transfer the entries from `PHOENIX_LLM_MODELS` to this file and remove
that environment variable from the repo env files **and the installed service
environment**. For macOS production, deploying once with the variable removed
updates the installed environment. Defining both sources is an error, including
`models = []` together with nonblank `PHOENIX_LLM_MODELS`; no precedence or merge
is implied. If there is no `models` section, the environment format remains usable.

Once migrated, edit this TOML file and run `./dev.py prod restart` on macOS.
Phoenix reads the file directly at startup, so later model edits require no
rebuild or redeployment. For development use `./dev.py restart`. Check
`phoenix-client.py --api-url <instance> --list-models` against the target instance
after restarting. Editing the file alone does not update the running model registry.

Workers/tiers and model definitions share the file but have separate loading
boundaries: creating a new parent runtime reloads worker preferences, not the
server model catalog. Invalid TOML, unsupported file versions, and invalid model
sections stop model-registry initialization with a path-specific diagnostic.

## Worker execution preferences

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
