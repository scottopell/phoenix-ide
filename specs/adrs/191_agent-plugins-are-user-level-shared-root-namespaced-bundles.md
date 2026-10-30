# ADR-191: Agent plugins are user-level-only, shared-root, namespaced bundles

- **Status:** Accepted
- **Date:** 2026-10-30
- **Affects:** REQ-AP-001 through REQ-AP-012

## Context

Phoenix needs a supported path to add and remove reusable capability bundles
— skills and MCP servers together — without waiting for either subsystem to
grow its own packaging story. The Agent Plugins specification 1.1.0 exists
precisely for this: a working-draft, cross-harness format standardizing a
`plugin.json` manifest, a `skills/` tree, and an `mcp.json` MCP configuration,
with install roots, naming, enable state, and lifecycle UX explicitly left to
clients. Phoenix already runs both component types — filesystem skills
(`phoenix-skills`) and MCP servers (`phoenix-mcp`) — so plugins are a
bundling and lifecycle layer, not a new execution path.

Three genuine forks shaped the decision:

1. **Install scope.** Project-level plugin discovery (the `.agent/plugins`
   convention some harnesses use) would let a repository checkout install
   executable subprocess configuration — MCP stdio servers — into the user's
   environment. A clone must never change which code runs in the harness.
2. **Install root.** A Phoenix-owned root (`~/.phoenix-ide/plugins/`) gives
   exclusive ownership; the spec's example root (`~/.agents/plugins/`) is
   shared with other harnesses installing the same format. The user
   prioritized cross-harness sharing.
3. **Naming.** Plugin skills and MCP servers enter namespaces that user
   skills and user MCP config already occupy. Collisions need a policy
   before the first plugin ships, not after the first shadowing bug.

The specification is a Working Draft: recognition semantics, failure
boundaries, and the runtime environment contract are normative today, but
the document can move. Version churn is a real risk to code written against
it.

## Options considered

1. **Phoenix-owned root, exclusive install** (`~/.phoenix-ide/plugins/`) —
   full lifecycle ownership, clean add/remove semantics, zero interference
   from other tools. Cost: plugins installed for Phoenix are invisible to
   other harnesses, duplicating installs for anyone using more than one.
2. **Shared spec-example root** (`~/.agents/plugins/`) — the root other
   harnesses reading the same spec would also choose, one install serving
   every conformant tool. Cost: Phoenix no longer owns the directory;
   another harness can add, update, or remove a plugin directory
   concurrently, so the registry and scanner must tolerate external churn
   and Phoenix must not treat shared-root state as authorization to
   destroy another tool's install or its data.
3. **Project-level discovery alongside user-level** — maximal convenience
   for per-repo tooling. Cost: the clone-changes-what-runs problem above;
   rejected on security grounds regardless of the root choice.

For naming:

1. **Bare plugin skill names, lowest precedence** — pretty invocations
   (`/deploy`), but two plugins shipping a `deploy` skill silently shadow
   each other, and the catalog cannot disambiguate them.
2. **Namespaced `plugin:skill`** — reuses the existing nested-skill
   precedent (`allium:distill`), makes cross-plugin collisions structurally
   impossible, and puts provenance in the invocation itself. Cost: slightly
   longer names.

## Decision

Plugins install and discover exclusively under the user-level shared root
`~/.agents/plugins/<name>/`, with per-plugin data at
`~/.agents/plugins/data/<name>/` (the spec's own environment example
shape). Option 2 for scope: the cross-harness sharing benefit is accepted
with the shared-root churn cost, managed by three rules: plugin identity is
the manifest `name` (not the directory), directories Phoenix did not
install load as `unmanaged` provenance and are never modified or removed
without an explicit request naming them, and plugin data survives remove
(purge is a separate explicit action) so no other harness's state is
destroyed by a Phoenix uninstall.

Plugin skills surface as `plugin:skill` (option 2 for naming), ranked below
every user skill source; plugin MCP servers key as `<plugin>.<server>`,
merged after all user configuration files so user configuration always
wins a collision. Namespacing is derived at discovery, never written into
frontmatter, so plugin trees stay byte-identical across harnesses.

Specification recognition is pinned to the exact canonical 1.1.0 schema
identifiers (`https://agent-plugins.org/schemas/1.1.0/{plugin,mcp}.schema.json`);
nothing else is recognized, no automatic same-major compatibility, and
schemas are never fetched while loading. A `mcp.json` whose declared version
differs from its `plugin.json` disables that plugin's MCP contribution — a
mixed-version package is invalid per spec, not a heuristic.

## Consequences

- **Positive:** One plugin directory serves every conformant harness; no
  repository can alter which code runs; user skills, user MCP config, and
  plugin contributions can never collide; plugin data survives updates and
  uninstalls; working-draft churn surfaces as an explicit
  unsupported-version report instead of silent misinterpretation.
- **Negative:** Phoenix must treat the plugins root as hostile-but-cooperative
  shared state — concurrent external modification is normal, so the scanner
  reports rather than fails, and destructive actions require provenance
  checks plus explicit user intent. The `mcp.json` → `McpServerConfig`
  translation adds a real mapping surface (transport tags, `${PLUGIN_*}`
  expansion, env overlay ordering) that must be tested against the spec's
  own examples. Plugin skill names are longer than bare names.
- **Neutral:** The `dev.scottopell.phoenix-ide/` extension namespace is
  reserved for future Phoenix-specific plugin content but ignored in v1,
  which is spec-conformant; the deprecated `sse` transport is skipped
  consistent with REQ-MCP-019's no-native-legacy-SSE decision.

## References

- Agent Plugins specification 1.1.0 —
  https://github.com/agentplugins/agent-plugins-spec/blob/main/spec/1.1.0.md
  (normative sections 1–11; install roots, naming, and lifecycle are
  client-defined)
- `specs/plugins/requirements.md`, `specs/plugins/plugins.allium`
- `specs/skills/` (discovery precedence, turn-scoped catalog), `specs/mcp/`
  (config merge, transport tagging, enable/disable persistence)
- Implementation anchors (planned): `phoenix-plugins` crate for manifest/
  component loading; `McpClientManager::read_all_configs` merge extension;
  `discover_all_skills_with_options` plugin source
