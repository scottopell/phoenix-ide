# ADR-269: Load external model definitions from user TOML at startup

- **Status:** Accepted
- **Date:** 2026-11-17
- **Affects:** REQ-LLM-003c, REQ-AG-010

## Context

Scott refreshes open-weight models independently of built-in GPT/Claude support.
The external catalog lives in `PHOENIX_LLM_MODELS` inside the repo environment
file. A macOS production restart deliberately preserves its installed environment,
so changing that repo file requires deployment to become effective. The version-1
XDG user TOML already holds worker and tier preferences and is read directly.

## Options considered

1. Reload the repo environment file during production restart. This changes the
   deployment environment contract for credentials, endpoints and unrelated settings.
2. Add external model definitions to the user TOML and read them at process startup.
   This supports edit-and-restart without changing deployment behavior.
3. Add live registry reload with a file watcher. This introduces model replacement
   and in-flight request questions unnecessary for the requested workflow.

## Decision

Choose option 2. Accept top-level `[[models]]` definitions using the external-model
schema and validator. Keep JSON environment definitions available when TOML has no
model section, but reject simultaneous sources, including an explicitly empty
TOML section. Operators migrate their installation explicitly; no automatic file
rewriting or persistent dual-source precedence is provided.

Each section has a typed consumer: the worker parser uses `IgnoredAny` for models,
and the model parser uses it for workers and tiers. Both recognize the same root
keys and version. Reuse the existing config-path resolver from phoenix-agents
rather than introducing another user-config location or moving the config system
into a new crate. Model entries share the existing external metadata conversion;
TOML rejects the entire section on invalid entries or duplicate external IDs.

Registry creation reads the user file. Parent-runtime recreation continues to
reload only workers and tiers. Authentication, endpoints, built-ins, and deployed
environment snapshots retain their existing ownership. A one-time production
deployment can remove the old installed environment variable; later TOML model
edits need only a restart. Older binaries are not promised to understand this
expanded TOML schema.

## Consequences

- **Positive:** Model catalog maintenance no longer requires deployment after migration.
- **Positive:** A conflict cannot silently mask refreshed definitions.
- **Negative:** Invalid TOML model configuration prevents registry initialization;
  it must be corrected before restarting successfully.
- **Negative:** The two contextual root readers must recognize any future shared
  top-level section; a combined workers/tiers/models test covers the shared shape.
- **Neutral:** The LLM crate depends on the lightweight agent/config crate for
  the existing XDG path resolver; there is no dependency back to the LLM crate.
- **Neutral:** Live reload and automatic migration remain outside the contract.

## References

- [LLM requirements](../llm/requirements.md)
- [Named-worker requirements](../agents/requirements.md)
- [ADR-052](052_workers-and-tiers-resolve-usable-model-routes.md)
- [Configuration workflow](../../docs/agents-config.md#external-model-definitions)
- `model_config::load_external_models`, `phoenix_agents::config_path`
