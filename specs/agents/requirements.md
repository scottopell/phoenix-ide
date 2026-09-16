# Named Agents

## User Story

As a user, I need reusable named workers and portable model preferences so the
LLM can delegate without guessing which providers this installation can use.
As an LLM, I need every advertised worker to have a usable default execution
choice, while retaining the option to choose a tier or an exact model route.

Named workers supply instructions, not authority. Spawn lifecycle, workspace
boundaries, fan-in, cancellation, and timeout are governed by
[`../subagents/requirements.md`](../subagents/requirements.md) and
[`../subagents/subagents.allium`](../subagents/subagents.allium).

## Requirements

Identifiers REQ-AG-001 through REQ-AG-003 are reserved. Their historical contracts
are preserved in [ADR-052](../adrs/052_workers-and-tiers-resolve-usable-model-routes.md#historical-requirements).
The user-configuration source contract is REQ-AG-010.

### REQ-AG-004: Agent Type as a Typed Spawn Choice

THE SYSTEM SHALL expose eligible named workers as an optional `agent_type`
enumeration on the `spawn_agents` per-task schema, with names and descriptions
AND SHALL allow anonymous tasks without an `agent_type`
AND SHALL NOT duplicate the worker catalog in system-prompt prose

WHEN a worker has no usable default execution choice or has an invalid
execution preference
THE SYSTEM SHALL exclude it from callable worker choices
AND SHALL expose an actionable configuration diagnostic

**Rationale:** An advertised worker must be usable without an override; a hidden
unavailable default must not turn a valid-looking batch into a predictable failure.

---

### REQ-AG-005: Spawn-Time Resolution and Precedence

WHEN a task selects an eligible named worker
THE SYSTEM SHALL use that worker's instructions as its persona

WHEN the task supplies an execution selector
THE SYSTEM SHALL resolve that selector independently of the worker's execution
preferences

WHEN execution is omitted and the worker declares execution candidates
THE SYSTEM SHALL select the first usable candidate in its declared order

WHEN execution is omitted and the task has no worker execution candidates
THE SYSTEM SHALL inherit the parent's exact model, connection, and effort

THE SYSTEM SHALL keep execution selection independent of requested authority

**Rationale:** A worker is a reusable persona with optional execution preferences;
explicit execution changes how it runs without discarding its instructions.

---

### REQ-AG-006: Persona Composition in the Sub-Agent System Prompt

WHEN a sub-agent is spawned from a named worker
THE SYSTEM SHALL replace the generic assistant preamble with its instructions
AND SHALL retain environment grounding and result-submission instructions

WHEN the child runtime is recreated during its run
THE SYSTEM SHALL restore the resolved persona from the child record
rather than resolving the worker definition again

**Rationale:** Configuration edits must not change an active child's persona.
This does not grant active children survival across server restart.

---

### REQ-AG-007: Unknown Agent Type Rejected

WHEN a task names a worker absent from the callable catalog used for its request
THE SYSTEM SHALL reject the whole spawn batch before any child is created
AND SHALL identify the unavailable name and callable alternatives

**Rationale:** Silent anonymous fallback would misrepresent the child's persona.

---

### REQ-AG-008: Catalog Snapshot Consistency

THE SYSTEM SHALL load worker and tier configuration once per parent runtime
AND SHALL refresh usable execution routes at each parent request boundary
AND SHALL use one resolved catalog snapshot for both that request's spawn schema
and admission of spawn calls from its response

THE SYSTEM SHALL render identical catalog content deterministically

WHEN a connection becomes unavailable after advertisement
THE SYSTEM SHALL report the unavailable route rather than silently selecting a
replacement outside the request's resolved choice

**Rationale:** Configuration is stable within a parent runtime; route availability
can change. Neither cache stability nor stale configuration may justify
advertising one choice and executing another.

---

### REQ-AG-009: Capability from Spawn Authority, Not Definition

THE SYSTEM SHALL derive child permissions and tools from requested authority and
the parent's workspace authority
AND SHALL NOT accept worker-owned mode, authority, or tool declarations

**Rationale:** Persona and cost preferences must not grant write access.

---

### REQ-AG-010: Single User Configuration

THE SYSTEM SHALL load named workers and tiers from the versioned user file
`$XDG_CONFIG_HOME/phoenix-ide/config.toml`, using `$HOME/.config` when
`XDG_CONFIG_HOME` is unset
AND SHALL use inline worker instructions, a description, and optional execution
candidates keyed by worker name
AND SHALL use this file as the sole named-worker source
AND SHALL NOT discover `.claude/agents` or `.agents/agents` definitions

WHEN the file is absent
THE SYSTEM SHALL offer anonymous spawning with parent inheritance and no
configured workers or tiers

WHEN configuration cannot be parsed, has an unsupported version, or contains
unsupported fields
THE SYSTEM SHALL produce an actionable diagnostic rather than interpreting it
as filesystem-agent configuration

**Rationale:** One explicit source prevents hidden local defaults and conflicting
catalogs. Skills carry reusable expertise; plugin hosting is a separate concern.

---

### REQ-AG-011: Ordered Atomic Execution Candidates

THE SYSTEM SHALL represent each worker or tier candidate as one model,
connection, and optional effort choice
AND SHALL preserve declared candidate order
AND SHALL skip candidates whose configured routes are unavailable

WHEN an available candidate requests an effort known to be incompatible
THE SYSTEM SHALL diagnose the affected worker or tier as invalid
AND SHALL exclude that selection from the callable catalog
rather than silently skipping the bad effort

WHEN a candidate omits effort
THE SYSTEM SHALL use that model's native default
AND SHALL NOT inherit effort from another candidate or the parent

THE SYSTEM SHALL advertise explicit effort choices only when supported by the
selected route's capability information

**Rationale:** Portability requires availability fallback, while a bad effort is
a configuration mistake. Keeping each choice atomic prevents cross-model effort
leakage.

---

### REQ-AG-012: Usable Model Routes

THE SYSTEM SHALL admit a model only through an enabled configured connection
with the credentials required by that connection and support for that model
AND SHALL preserve the selected connection identity through child creation and
runtime recreation

THE SYSTEM SHALL NOT infer availability from a provider family name or from
Phoenix knowing a model identifier
AND SHALL NOT restrict children to the parent's provider when other usable
connections exist

WHEN the selected connection cannot be used
THE SYSTEM SHALL report failure without silently rerouting the child

WHEN a user explicitly changes a child's model through the conversation model
change operation
THE SYSTEM SHALL update its selected connection together with the model and effort
AND SHALL preserve its resolved instructions

**Rationale:** A Codex-only installation must not advertise Anthropic choices.
Connection identity names the configured backend slot; it is not a guarantee
that operator changes preserve an account or endpoint.
