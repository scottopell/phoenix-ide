# Project Instruction Snapshots

## User Story

As a user, I need an active conversation to keep following the project instructions I reviewed, even when repository guidance or installed skills change, so agent behavior and prompt-cache use do not change silently.

As a user, I need to see which instruction sources changed and deliberately refresh them, so I can trade a known one-time cache rewarm for updated behavior without losing conversation history.

## Requirements

### REQ-PI-001 — Stable Active Bundle

WHEN a conversation prepares a model request
THE SYSTEM SHALL use one immutable project-instruction bundle containing the applicable `AGENTS.md` / `AGENT.md` guidance and each skill's catalog metadata, exact frontmatter-stripped `SKILL.md` body, stable source path, and base directory
AND SHALL keep that bundle unchanged throughout a user turn and its tool loop.

WHEN either the user invokes a slash skill or the model invokes the Skill tool in an existing conversation
THE SYSTEM SHALL render the invocation from the bundle governing that invocation's turn, using its captured skill name, body, and base directory without rediscovering or rereading `SKILL.md`.

A direct user message that starts the next turn, and steering known to start a later turn, SHALL expand slash skills from the queued bundle when one is waiting; invocation within the active turn SHALL expand from the active bundle.

Skill companion files remain live resources outside the snapshot and MAY be read through ordinary file tools after invocation.

The system SHALL keep live runtime state, including conversation mode, permissions, and tool availability, outside the project-instruction bundle.

**Rationale:** Project files can change during agent work. Re-reading them for every model request silently changes behavior and invalidates a large cached prefix, while freezing runtime authority could apply obsolete permissions.

### REQ-PI-002 — Explicit Refresh

WHEN instruction sources differ from the active or already-queued bundle
THE SYSTEM SHALL indicate that newer project instructions are available
AND SHALL NOT apply them until the user confirms **Refresh project instructions**.

New top-level conversations SHALL resolve the latest applicable sources for their initial bundle.

WHEN a sub-agent is spawned
THE SYSTEM SHALL create a new active immutable bundle by copying the parent's persisted active bundle exactly, including ordered guidance, skill metadata and bodies, and token estimate,
AND SHALL assign the child bundle a new identity,
AND SHALL complete that copy before the child runtime can make its first model request,
AND SHALL NOT discover project instructions independently for the child.

IF the parent predates project-instruction snapshots
THEN THE SYSTEM SHALL initialize the parent once from the parent's working directory before copying its active bundle to the child.

For asynchronously provisioned conversations, only the creation worker SHALL initialize the initial bundle, after it has finalized the conversation's effective working directory. The metadata/mode update and initial-bundle insertion SHALL be one transaction fenced by the worker identity, claim token, generation, unexpired lease, and expected stage; a stale claimant SHALL NOT insert a bundle. The committed bundle SHALL govern initial slash-skill expansion, including seeded-empty and expansion-preflighted creation paths. While the creation job is not `Ready` — including failed, cancelled, and deletion-pending states — conversation-scoped project-instruction, system-prompt inspection, and skill-catalog endpoints SHALL return a typed unavailable/conflict response and SHALL NOT discover or persist instructions from the provisional working directory.

### REQ-PI-003 — Source Manifest

WHEN the system presents a refresh candidate
THE SYSTEM SHALL identify each changed guidance source by conversation-working-directory-relative path and `added`, `changed`, or `removed` status
AND SHALL identify changed skill-catalog entries separately by skill name and status
AND SHALL NOT display guidance-file contents in the manifest.

Unchanged sources SHALL be summarized or collapsed by default.

The conversation-scoped refresh preview endpoint SHALL discover current sources, persist the exact candidate represented by its response, compare it with the queued bundle when one exists and otherwise with the active bundle, and return only bundle identities, source-change metadata, and estimate-labeling data.

The conversation-scoped status endpoint SHALL discover current sources and compute its manifest transiently, SHALL NOT create or replace a candidate, and SHALL report the identity of any existing candidate unchanged.

The conversation-scoped confirmation endpoint SHALL accept a candidate bundle identity and SHALL reject the request when that exact candidate is missing or has been replaced.

### REQ-PI-004 — Cache-Rewarm Estimate

WHEN the system presents a refresh candidate
THE SYSTEM SHALL show an estimated one-time input-token rewarm size
AND SHALL label the value as an estimate whose actual provider cache behavior may differ.

**Rationale:** Refreshing a large leading prefix can consume materially more quota on the next request, so the user needs the likely cost before confirming.

### REQ-PI-005 — Exact Confirmation

WHEN the user confirms a refresh candidate
THE SYSTEM SHALL preserve the exact normalized bundle represented by the reviewed manifest
AND SHALL NOT replace it with later filesystem contents before activation.

IF sources change again after confirmation
THEN THE SYSTEM SHALL keep the confirmed bundle queued and expose the newer sources as another refresh opportunity.

### REQ-PI-006 — Turn-Boundary Activation

WHEN a confirmed bundle is waiting to activate
THE SYSTEM SHALL finish any active user turn and complete tool loop under the existing bundle
AND SHALL activate the confirmed bundle before processing the next user-authored turn.

A direct user message and a steering message drained while entering idle SHALL each start such a boundary. A steering drain during an existing tool loop SHALL NOT activate the bundle.

Activation SHALL atomically preserve conversation history, persist a visible content-free System instruction-refresh timeline event, advance the transcript generation, and invalidate incompatible provider continuation state. The system SHALL emit the durable event after commit, and System timeline events SHALL remain excluded from model context.

A rejected user message SHALL NOT activate the queued bundle, advance transcript generation, or add an activation timeline event.

Each accepted user-authored turn SHALL be bound to the exact queued bundle used for slash expansion, or explicitly to the active bundle when no queue existed. Activation SHALL compare-and-swap that expected bundle ID before any turn effects. If the queue was removed or replaced, the system SHALL reject the turn rather than execute an expansion from one bundle under another bundle, and SHALL emit a sequenced resynchronization error without leaving an SSE sequence gap.

### REQ-PI-007 — Durable Recovery

WHEN Phoenix restarts
THE SYSTEM SHALL recover the exact active, queued, and newer candidate bundles, including captured skill bodies, invocation paths, and argument hints,
AND SHALL preserve the same activation boundary and source manifest.

The system-prompt inspection surface SHALL render the persisted active bundle rather than rediscovering mutable filesystem sources.

The conversation-scoped skill-catalog endpoint SHALL return skill metadata, including argument hints, from the persisted active bundle rather than rediscovering mutable filesystem sources. The directory-scoped project skill endpoint used before conversation creation SHALL remain live discovery of the requested project directory.

WHEN a conversation predates project-instruction snapshots
THE SYSTEM SHALL resolve and persist its initial bundle before its next model request without dropping or rewriting conversation history.
