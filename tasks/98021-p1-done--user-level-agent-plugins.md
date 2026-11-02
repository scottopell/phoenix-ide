# User-Level Agent Plugins (Agent Plugins Spec 1.1.0)

## Goal

Support a native, user-level-only (never project-level) add/remove path for
plugins, adhering to the Agent Plugins specification 1.1.0. A plugin bundles
two component types Phoenix already runs: skills and MCP servers.

Before starting: read the roadmap issue (#806) body for related outcomes.

## Spec reality: what "adhere to 1.1.0" means

The portable surface is exactly three things:

- `<plugin-root>/plugin.json` — manifest, exact field set, `name` pattern
  `^(?!.*(?:--|\.\.))[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$`
- `<plugin-root>/skills/<dir>/SKILL.md` — Agent Skills format, immediate
  children only (non-recursive)
- `<plugin-root>/mcp.json` — MCP servers, stdio or streamable-http; the
  deprecated `sse` transport may be skipped (consistent with REQ-MCP-019)

Everything else — install roots, skill naming/collision policy, enabled
state, install/uninstall UX, marketplaces — is deliberately client-defined.
The spec is a Working Draft; recognition pins to the exact canonical
`$schema` identifiers and never fetches schemas while loading.

Failure boundaries (each row needs a test):

| Condition | Required scope |
|---|---|
| Manifest missing, invalid, or unsupported `$schema` version | Reject whole plugin |
| Unknown top-level manifest field | Report and ignore; continue |
| Non-object `extensions` | Report and ignore; continue |
| Unimplemented extension namespace | Ignore without validating contents |
| Component location wrong filesystem kind / escapes root | Invalidate that component type; continue others |
| Invalid or escaping `SKILL.md` | Skip that skill; continue |
| Invalid `mcp.json` (structure, version mismatch with `plugin.json`) | Disable that plugin's MCP; skills still load |
| Invalid individual server | Skip that server; continue |
| Unsupported transport | Skip that server; continue |

Runtime contract for stdio servers:

- Supply `PLUGIN_ROOT` and `PLUGIN_DATA`; client-owned values override any
  equivalent names last.
- Single-pass `${PLUGIN_ROOT}` / `${PLUGIN_DATA}` expansion in each `args`
  element, each `env` value, and `cwd`. Never expand `command` or `env` keys.
  No recursive or other expansion.
- Configured `env` may not declare `PLUGIN_ROOT`/`PLUGIN_DATA`.
- `cwd` defaults to the plugin root; explicit values must be `./`-relative or
  `${PLUGIN_ROOT}`/`${PLUGIN_DATA}`-anchored and resolve inside the boundary.
- `command` is one executable token: bare name (PATH lookup) or plugin-
  relative `./` path. Bundled executables must use `./`.
- `PLUGIN_DATA`: create before launch, make writable, preserve across plugin
  update; deletion on uninstall is optional (client choice).
- Containment: resolve real paths including symlinks; accessed package
  paths must stay inside the resolved plugin root (`PLUGIN_DATA` has its
  own boundary). Escapes are rejected.

## Approved decisions

1. Install root `~/.agents/plugins/<name>/`; `PLUGIN_DATA` at
   `~/.agents/plugins/data/<name>/` (the spec's own example shape). This root
   may be shared with other harnesses — that is the accepted tradeoff.
2. Plugin skills surface as `plugin:skill` (e.g. `devtools:deploy`) in both
   catalog and invocation, lowest precedence under user skills. The spec
   mandates no collision policy; this is a client choice matching the
   existing nested-skill precedent (`allium:distill`). Frontmatter `name`
   keeps matching its parent directory; the prefix is derived at discovery,
   never rewritten.
3. Plugin MCP servers key as `<plugin>.<server>`, merged after all user
   config files (user config wins collisions, first-seen-wins). Tools keep
   surfacing as `{server}__{tool}`.
4. User-level only: no project-relative plugin root discovery, ever.
5. Plugin skills are ordinary filesystem skills: audience stays Conversation.
   REQ-SK-008 already structurally prevents filesystem skills from claiming
   GlobalCoordinator visibility — no new machinery.
6. Extension namespace reserved: `dev.scottopell.phoenix-ide/`. v1 ignores
   all extension namespaces (spec-conformant).
7. Shared-root hygiene: plugin directories Phoenix did not install are
   "unmanaged" — loaded read-only, no destructive actions without explicit
   user intent. Uninstall removes the plugin directory; `PLUGIN_DATA` is
   retained by default, purged only by an explicit separate action.
8. Plugin identity = manifest `name`, not directory name. Two plugin roots
   declaring the same name: first-seen-wins with a reported diagnostic
   (MCP config merge precedent). Missing root for an enabled plugin: report,
   do not crash.

## Phases

### Phase 0 — Specs (gates everything else)

- `specs/plugins/requirements.md` (REQ-AP-xxx): user-level install scope,
  installed-plugin discovery, enabled/disabled semantics, component loading
  with the failure-boundary matrix, containment, env/expansion contract,
  uninstall semantics, status surfacing.
- `specs/plugins/plugins.allium`: lifecycle states
  (not-installed → installed{enabled|disabled} → removing), the
  failure-boundary matrix as rules, env construction ordering.
- One ADR: user-level-only scope, shared install root, naming scheme
  (`plugin:skill`, `<plugin>.<server>`), `PLUGIN_DATA` retention policy,
  Working-Draft version pinning.
- Run the spec pre-flight checklist (specs/AUTHORING.md) before pushing.

### Phase 1 — `crates/phoenix-plugins` (pure, no runtime wiring)

- Manifest model: exact field set with the spec's ignore/report/reject
  semantics; the `name` regex.
- `mcp.json` model: closed stdio / streamable-http variants; `sse` recognized
  but skipped; per-server validation independent of the document.
- Pure functions: placeholder expansion (single pass, correct target fields),
  env overlay ordering with client-owned `PLUGIN_*` last, containment checks
  (realpath, both boundaries).
- Unit tests built from the spec's own examples, one per failure-boundary
  row.

### Phase 2 — MCP runtime integration

- `McpServerConfig::Stdio` gains `cwd`; `stdio.rs` spawn sets
  `current_dir`. Resolved values stay stable across respawn (REQ-MCP-003).
- Transport translation: `streamable-http` maps to Phoenix's existing HTTP
  transport; `headers` from `mcp.json` map through.
- Plugin servers feed into the `read_all_configs` merge after user files,
  keyed `<plugin>.<server>`.
- `PLUGIN_DATA`: create-before-spawn, writable, preserved across update.
- Reload reconciliation reports plugin server add/remove/restart
  (REQ-MCP-015 extends).
- `specs/mcp` requirements updated where behavior changes (`cwd`, plugin
  config source) — normative change, not silent hardening.

### Phase 3 — Skills integration

- Discovery reads enabled plugins' `skills/` directories: immediate children
  only (spec non-recursive). Phoenix's recursive sub-skill extension stays
  confined to non-plugin directories.
- Namespace `plugin:skill`, lowest precedence; dedup interplay (same file
  through two roots counts once) follows existing rules.
- Catalog fragment marks plugin provenance; refresh rides existing REQ-SK-009
  turn boundaries — no new machinery.
- `specs/skills` requirements updated for the plugin source.

### Phase 4 — Lifecycle UX

- SQLite registry: plugin identity, enabled/disabled, provenance
  (managed/unmanaged). Columns per the persisted-structure rules — no JSON
  blobs. Mirrors the `mcp_disabled_servers` precedent.
- API: `/api/plugins` list/status/add/remove/enable/disable. Add accepts a
  local path or a git URL in v1; nothing more.
- UI: Plugins page (sibling to skills-ui), information-density style,
  inline status indicators.
- `phoenix-client.py`: `plugins` subcommands matching the API.

## Acceptance criteria

- Every failure-boundary row has a test.
- A plugin at `~/.agents/plugins/devtools/` with skills and `mcp.json`
  becomes usable: skills via `/devtools:deploy` and the Skill tool; MCP
  tools via live resolution (REQ-MCP-014).
- A disabled plugin contributes nothing; re-enable restores skills next
  turn (REQ-SK-009) and MCP live.
- Remove deletes the plugin directory and retains `PLUGIN_DATA`; purge is
  a separate explicit action.
- No project-level plugin discovery exists anywhere.
- `./dev.py check` passes.

## Risks

- Working Draft churn: mitigated by exact-`$schema` recognition and the
  `mcp.json`/`plugin.json` version-match rule.
- Shared root: external tools may add/remove plugin dirs concurrently;
  registry keyed on manifest name tolerates churn (missing root → report).
- The stdio spawn/respawn path is touched; keep resolved `cwd`/env stable
  across respawn.
