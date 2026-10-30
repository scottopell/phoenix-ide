# User-Level Agent Plugins

## User Story

As a user, I need to add, remove, enable, and disable reusable plugin bundles
at the user level, so that skills and MCP servers installed once are available
across all my conversations and repositories without any project opt-in or
per-project copies.

As a plugin author, I need to distribute one directory layout that follows
the Agent Plugins specification 1.1.0, so that the same plugin works in
Phoenix and other conformant harnesses without adaptation.

As a user, I need a repository to be unable to add, remove, or modify plugins
on its own, so that no checkout or clone can change which code runs in my
harness.

## Background

A plugin is a directory containing a `plugin.json` manifest, an optional
`skills/` component tree, and an optional `mcp.json` MCP configuration. The
Agent Plugins specification 1.1.0 standardizes exactly those three surfaces;
install roots, enable/disable state, naming policy, and lifecycle UX are
client-defined. Phoenix consumes the portable surface and owns the lifecycle
around it.

Phoenix already runs both component types: filesystem skills
(`specs/skills/`) and MCP servers (`specs/mcp/`). Plugins are a bundling and
lifecycle layer over the two; component behavior after loading is governed
by the owning specs, not duplicated here.

## Requirements

### REQ-AP-001: User-Level Install Scope

THE SYSTEM SHALL install and discover plugins exclusively under a
user-level plugins root derived from the user's home directory
THE SYSTEM SHALL NOT discover plugins from any working directory, project
ancestor, or repository-relative path
AND SHALL NOT expose any project-level install, enable, or disable path

**Rationale:** A repository checkout must never change which code runs in the
harness. Project-level plugin discovery would let a clone add executable
subprocess configuration (MCP servers) to the user's environment without
explicit user action. The Agent Plugins specification leaves install roots
client-defined; user-level-only is the conformant choice that closes this
attack surface entirely.

---

### REQ-AP-002: Installed-Plugin Discovery

THE SYSTEM SHALL treat each immediate child directory of the plugins root as
a candidate plugin root and SHALL load a candidate only when its
`<plugin-root>/plugin.json` resolves to a regular file and parses as a valid
manifest
WHERE a child directory carries no valid manifest THE SYSTEM SHALL report it
at `debug` level and continue with siblings
WHERE the plugins root itself is missing THE SYSTEM SHALL treat the plugin
set as empty without error

THE SYSTEM SHALL NOT recurse into nested directories searching for manifests

WHERE two plugin roots in the same scan declare the same manifest `name` THE
SYSTEM SHALL load the first discovered and SHALL report the duplicate at
`warn` level

Plugin identity SHALL be the manifest `name`, not the directory name.

**Rationale:** The spec's environment example puts plugin roots as immediate
children of a shared root (`~/.agents/plugins/devtools`); one-level scanning
matches that shape and keeps discovery bounded. The root is shared with other
harnesses, so directories Phoenix did not install (or intermediate state from
another tool's install) must not break the scan. Identity by manifest name
tolerates a directory being renamed out from under the registry.

---

### REQ-AP-003: Manifest Validation

THE SYSTEM SHALL validate `plugin.json` against the 1.1.0 manifest schema:
required `$schema` exactly equal to a client-recognized canonical identifier,
required `name` matching
`^(?!.*(?:--|\.\.))[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$`, and optional
`version`, `description`, `author`, `homepage`, `repository`, `license`,
`keywords`, `extensions` of the documented types

WHERE a manifest carries an unknown top-level field THE SYSTEM SHALL report
the field at `warn` level and load the plugin otherwise unimpaired, giving the
field no semantics
WHERE `extensions` is present but is not an object THE SYSTEM SHALL report
the field at `warn` level and ignore it entirely
WHERE an `extensions` namespace is present but unimplemented THE SYSTEM
SHALL ignore that namespace without validating its contents

WHERE any other manifest violation occurs -- including an unrecognized
`$schema` identifier -- THE SYSTEM SHALL reject the entire plugin and load
none of its components

THE SYSTEM SHALL NOT reject a manifest merely because `version` is not valid
SemVer, a URL-like field is not a recognized URL, `author.email` is not a
recognized email, or `license` is not an SPDX identifier
AND SHALL reject type violations of those fields (a numeric `version`, a null
`description`, an unexpected `author` property)

THE SYSTEM SHALL NOT fetch any schema over the network while loading a
plugin

**Rationale:** These are the 1.1.0 loader rules verbatim: lenient on metadata,
strict on structure, unknown fields carry no semantics, and offline loading
is mandatory. Recognition is pinned to the canonical
`https://agent-plugins.org/schemas/1.1.0/plugin.schema.json` identifier so
working-draft churn surfaces as an explicit unsupported-version report
rather than silent misinterpretation.

---

### REQ-AP-004: Component Loading with Scoped Failure Boundaries

THE SYSTEM SHALL load the two portable component types from a valid plugin:

- skills from `<plugin-root>/skills/`, each immediate child directory whose
  exact `SKILL.md` path resolves to a regular file, non-recursively
- MCP servers from `<plugin-root>/mcp.json`, when that path is a regular
  file

WHERE a component location exists with the wrong filesystem kind (a regular
file named `skills`, a directory named `mcp.json`) THE SYSTEM SHALL invalidate
that component type for that plugin and continue loading the other type

WHERE a skill's `SKILL.md` is invalid or escapes the plugin root THE SYSTEM
SHALL skip that skill, report at `warn` level, and continue sibling skills

WHERE `mcp.json` is missing, its JSON is invalid, its top-level structure is
invalid, its declared specification version differs from the manifest's, or
it declares an unsupported specification version THE SYSTEM SHALL disable
that plugin's MCP contribution and continue loading skills

WHERE an individual server entry in a valid `mcp.json` is invalid, uses an
unsupported transport, or resolves outside the plugin root THE SYSTEM SHALL
skip that server, report at `warn` level, and continue sibling servers

WHERE a server's `command` is neither a bare executable name nor a
plugin-relative path beginning `./` THE SYSTEM SHALL skip that server

**Rationale:** Failure scope is per-component, never all-or-nothing beyond the
manifest: a broken MCP bundle must not take the plugin's skills down with
it, and one bad server entry must not disable its siblings. These are the
1.1.0 loader failure boundaries; each row is separately testable.

---

### REQ-AP-005: Plugin Skills

THE SYSTEM SHALL surface each enabled plugin's skills under the namespaced
name `<plugin>:<skill>` in both the skill catalog and invocation, where
`<plugin>` is the manifest name and `<skill>` is the frontmatter name

THE SYSTEM SHALL derive the namespace prefix at discovery time and SHALL
NOT require, rewrite, or store it in the skill's frontmatter

WHERE the same namespaced plugin skill name would arise from multiple sources
THE SYSTEM SHALL resolve it by the existing skill discovery precedence rules
(`specs/skills/` REQ-SK-006), with plugin skills ranking below every user
skill source

WHERE a plugin's `skills/` tree contains nested sub-skill directories beyond
the immediate children THE SYSTEM SHALL ignore them

Plugin skills SHALL be ordinary filesystem skills for all existing skill
requirements: frontmatter separation, delivery semantics, base-directory
context, argument substitution, shared expansion, and turn-scoped catalog
advertisement (`specs/skills/` REQ-SK-001 through REQ-SK-005, REQ-SK-009)

THE SYSTEM SHALL mark plugin skills' provenance in the catalog so the model
can distinguish them from user skills and built-ins

**Rationale:** The spec mandates no collision or naming policy -- it defers
skill format to the Agent Skills specification and exposes no portable
invocation surface -- so naming is a client decision. The `plugin:skill`
scheme reuses the existing namespaced-skill precedent (`allium:distill`) and
makes cross-plugin collisions structurally impossible. Non-recursive loading
is the 1.1.0 rule; Phoenix's recursive sub-skill extension is confined to
non-plugin directories so plugin trees stay portable.

---

### REQ-AP-006: Plugin MCP Servers

THE SYSTEM SHALL key each enabled plugin's MCP servers as
`<plugin>.<server>` and merge them into the MCP configuration after every
user configuration file, preserving the first-seen-wins merge so user
configuration always wins a name collision

THE SYSTEM SHALL translate the 1.1.0 transport tags to the transport-tagged
config model (`specs/mcp/` REQ-MCP-001): `stdio` and `streamable-http` map
through; the deprecated `sse` transport is skipped as unsupported, consistent
with REQ-MCP-019

A plugin stdio server SHALL run with:

- `PLUGIN_ROOT` set to the resolved plugin root and `PLUGIN_DATA` set to the
  plugin's dedicated data directory, both client-owned and overriding any
  equivalent configured names last
- a working directory defaulting to the plugin root, where any explicit
  `cwd` is `./`-relative or `${PLUGIN_ROOT}`/`${PLUGIN_DATA}`-anchored and
  resolves inside its anchor boundary
- single-pass `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` expansion applied to each
  `args` element, each `env` value, and `cwd`, and to nothing else
- configured `env` values that do not declare `PLUGIN_ROOT` or `PLUGIN_DATA`

WHERE a configured `env` entry declares `PLUGIN_ROOT` or `PLUGIN_DATA` THE
SYSTEM SHALL invalidate that server

THE SYSTEM SHALL create the plugin's data directory before first launch,
maintain it writable, and preserve its contents across plugin updates; update
MUST NOT delete or reset it

Plugin server tools SHALL surface to conversations through the existing
live-resolution tool exposure (`specs/mcp/` REQ-MCP-014) under the
`{server}__{tool}` naming

**Rationale:** Keying as `<plugin>.<server>` keeps tool names unambiguous
without new machinery and gives user config a structural precedence over
plugins. The environment and expansion contract is the 1.1.0 runtime
requirement verbatim: one textual pass over the designated fields, no other
expansion, client-owned variables last, data directory preserved across
updates so a plugin update never destroys plugin state.

---

### REQ-AP-007: Containment

THE SYSTEM SHALL resolve the real filesystem paths of plugin-supplied files
and directories, following symlinks, and SHALL reject any package-supplied
path that resolves outside the resolved plugin root (outside the data
directory, for data-anchored paths)

WHERE a component path, `cwd`, or plugin-relative executable escapes its
boundary THE SYSTEM SHALL reject that component or server, report at `warn`
level, and continue loading siblings

THE SYSTEM SHALL NOT treat argument strings or environment values as
package paths for containment enforcement; they are opaque

**Rationale:** A plugin bundle is untrusted input. The 1.1.0 security rules
require resolved-path containment, including symlink escapes; links pointing
inside the root are allowed. The distinction between enforced package paths
and opaque strings keeps containment from becoming a broken validator over
arbitrary text.

---

### REQ-AP-008: Enable and Disable

THE SYSTEM SHALL let a plugin be disabled so that none of its components are
loaded, and enabled again, without reinstall

THE SYSTEM SHALL persist the enabled/disabled set across restarts in the
SQLite operator-state database, as columns and rows rather than a JSON blob

WHERE a plugin is disabled THE SYSTEM SHALL NOT spawn its MCP servers and
SHALL NOT include its skills in any catalog, and any existing connection to
its servers SHALL be disconnected through the existing reload reconciliation

THE SYSTEM SHALL treat a plugin discovered in the plugins root without a
registry record as enabled by default

**Rationale:** Default-enabled matches how filesystem skills behave today:
putting a directory in the discovery path is the act of installing. The
registry only records deviations and lifecycle state, mirroring the
`mcp_disabled_servers` precedent. MCP connections ride the existing
reconciliation so disabling a plugin cannot orphan a running subprocess.

---

### REQ-AP-009: Add and Remove Lifecycle

THE SYSTEM SHALL provide an add path that installs a plugin from a local
path or a git URL into a plugin directory under the plugins root, and a
remove path that uninstalls it

WHERE an added plugin's manifest name collides with an already-loaded
plugin THE SYSTEM SHALL reject the add with a diagnostic

WHERE the plugins root contains a plugin directory Phoenix did not install
THE SYSTEM SHALL load it like any other plugin and mark its provenance as
unmanaged, and SHALL NOT remove, overwrite, or modify it without an explicit
remove request naming it

THE SYSTEM SHALL NOT delete a plugin's data directory on remove; data
deletion is a separate explicit action

WHERE an installed plugin's root is missing at discovery time THE SYSTEM
SHALL report the plugin as missing, retain its registry record, and continue

**Rationale:** The root is shared with other harnesses, so another tool may
install, update, or remove plugin directories concurrently. Unmanaged
directories are still loadable -- they follow the same format -- but Phoenix
must never treat shared-root churn as authorization to delete another tool's
install. Data retention on remove is the conservative default: destroyed
plugin state is unrecoverable, and the 1.1.0 specification leaves deletion on
uninstall as a client option.

---

### REQ-AP-010: Status Surfacing

THE SYSTEM SHALL expose, per plugin: name, description, version, provenance
(managed or unmanaged), enabled state, per-component load outcomes (skills
loaded or skipped with reasons, MCP servers loaded, skipped, or disabled with
reasons), and missing-root state for registered-but-absent plugins

**Rationale:** A plugin that half-loads (skills present, `mcp.json` invalid)
must be legible in the UI the same way a failed MCP server is
(`specs/mcp/` REQ-MCP-018). Failure visibility is what makes the
debug-to-retry loop possible; silent omission would be indistinguishable
from a bug.

---

### REQ-AP-011: Extension Namespaces Are Reserved but Inert

THE SYSTEM SHALL reserve the reverse-domain namespace
`dev.scottopell.phoenix-ide/` for Phoenix-specific plugin extensions and
SHALL ignore all extension namespaces, manifest `extensions` entries, and
extension directories without interpretation

**Rationale:** The 1.1.0 portable surface is skills plus MCP; extension
namespaces are the sanctioned escape hatch for client-specific behavior.
Reserving the name now prevents a later collision; ignoring them is
spec-conformant and keeps the v1 loading surface minimal.

---

### REQ-AP-012: Version Recognition

THE SYSTEM SHALL recognize exactly the canonical 1.1.0 schema identifiers
for both `plugin.json` and `mcp.json`, mapping each to one implementation
THE SYSTEM SHALL reject a plugin whose manifest declares an unrecognized
specification version, reporting the version
WHERE a plugin's `mcp.json` declares a different specification version than
its `plugin.json` THE SYSTEM SHALL disable the plugin's MCP contribution
(even when both versions are individually supported)

**Rationale:** The 1.1.0 recognition rule is explicit: no automatic
same-major or nearest-version compatibility, and a mixed-version package has
invalid MCP configuration. Pinning recognition to exact identifiers means a
future 1.2 draft surfaces as a clear "unsupported version" report, never as
silently misinterpreted fields.
