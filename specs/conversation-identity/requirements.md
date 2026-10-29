# Conversation Identity and Naming

## User story

As a person using Phoenix, I need one editable, readable conversation name and a readable current URL, without renaming changing the conversation or invalidating exact historical evidence.

## Scope

An ordinary ProductConversation is the durable conversation. Its parent transcripts, continuation records, subordinate executions, messages, and WorkScopes retain their own identities. Naming introduces neither another aggregate nor another lifecycle.

## Requirements

### REQ-CID-001 — One human name

THE SYSTEM SHALL use the canonical root transcript's `title` as the sole ProductConversation name authority.

THE SYSTEM SHALL accept names containing full words, spaces, and Unicode characters, preserve their case and text apart from outer whitespace trimming, and allow different ProductConversations to have the same name.

THE SYSTEM SHALL NOT expose a separate editable slug or chain-name authority.

THE SYSTEM SHALL expose manual title changes and generated-name changes through the ProductConversation naming operations, and SHALL NOT support legacy chain-name mutation routes as alternate callers.

WHEN a generated name is accepted, THE SYSTEM SHALL store human-readable prose rather than a kebab-case substitute for that name.

### REQ-CID-002 — Allocated route projection

WHEN a ProductConversation name is created or changed, THE SYSTEM SHALL atomically store its name and a unique route projection in the root transcript's existing `slug` column.

THE SYSTEM SHALL derive a lowercase hyphen-separated projection from the name's Unicode letters and numbers, use `conversation` if there are no such characters, and resolve collisions with the first available candidate among the base, `base-2`, `base-3`, and subsequent integer suffixes.

THE SYSTEM SHALL allocate against the shared namespace of all transcript slugs and all actual transcript and ProductConversation IDs, including durably reserved creation identities, without imposing a UUID-shaped-name prohibition or using shortened IDs.

THE SYSTEM SHALL serialize allocation and mutation in the same database write transaction, so concurrent names cannot reserve the same projection or expose a new name with an old projection.

### REQ-CID-003 — Current readable navigation

THE SYSTEM SHALL publish the current canonical conversation route as `/c/<allocated-root-slug>`, percent-encoding the UTF-8 slug as one path component.

THE SYSTEM SHALL provide this route consistently in aggregate snapshots, list rows, published creation responses, and newly produced ordinary conversation links.

THE SYSTEM SHALL keep the permanent ProductConversation UUID as the API, cache, and durable-reference identity; a readable route SHALL NOT replace that identity.

### REQ-CID-004 — Continuation and exact evidence

WHEN a conversation continues, THE SYSTEM SHALL retain the aggregate root's name and projection without requiring another naming action.

WHEN a name changes, THE SYSTEM SHALL preserve ProductConversation, transcript, continuation, message, and exact evidence-reference identities.

THE SYSTEM SHALL resolve a permanent ProductConversation ID or an exact transcript/message reference independently of mutable slugs.

THE SYSTEM SHALL NOT promise retention, redirection, or reservation of a former slug after rename. A released slug MAY be allocated to another conversation; it SHALL NOT remain an alias for the renamed conversation.

### REQ-CID-005 — Existing admission and async identity

THE SYSTEM SHALL apply existing History, active Close, and creation-claim admission rules to name and projection writes in their committing transaction, without introducing another authorization or lifecycle policy.

WHEN name generation or another asynchronous operation starts from a route reference, THE SYSTEM SHALL pin the owning immutable identity before awaiting external work and recheck admission at commit, rather than resolve a possibly reassigned slug at commit.

WHEN an asynchronous generated name is ready, THE SYSTEM SHALL commit it only if the root naming revision still matches the immutable-owner baseline captured before generation; a newer naming operation SHALL NOT be overwritten by that stale result.

WHEN metadata publication loses its creation claim, THE SYSTEM SHALL commit neither a name nor a slug from that stale worker.

### REQ-CID-006 — Forward transformation

WHEN the name-authority migration runs, THE SYSTEM SHALL preserve each root's effective existing name by folding its nonempty `chain_name`, otherwise its nonempty `title`, otherwise its nonempty `slug`, otherwise `Conversation`, into the root's `title`.

THE SYSTEM SHALL allocate root projections, remove the competing persisted `chain_name` column and domain field, and preserve stable identities and message content.

THE SYSTEM SHALL NOT introduce a second ProductConversation name column, an old-slug registry, a persistence framework, or an additional naming lifecycle.

### REQ-CID-007 — Coordinator Rename Is Fenced and Replayable

WHEN the current Global Coordinator renames an ordinary ProductConversation through its dedicated naming capability
THE SYSTEM SHALL require the immutable product identity, canonical root transcript identity, and naming revision returned by a fresh detail read
AND SHALL commit the shared title and allocated-slug update only if that exact naming baseline and existing lifecycle admission still hold.

THE SYSTEM SHALL persist the normalized rename intent, trusted executing source-call identity, and resulting naming receipt under one operation identity in the same transaction as the naming update
AND SHALL reconcile identical retries from that historical receipt without reapplying the rename, even after a later naming change
AND SHALL reject changed intent or source identity reuse rather than silently choose another operation or target
AND SHALL distinguish a historical receipt from the current name, route, and lifecycle state.

THE SYSTEM SHALL retain the existing immutable identity and released-slug semantics without reserving former routes or granting lifecycle mutation authority.

## Acceptance scenarios

- Duplicate human names remain identical while projections allocate `base`, `base-2`, and `base-3` under concurrent rename and creation.
- A failed or fenced rename changes neither name nor projection; History and active Close remain protected.
- A Unicode name remains readable in presentation while its current canonical route is correctly encoded; punctuation-only names use the fallback projection.
- A name equal to an actual UUID or other persisted ID gets a safe collision suffix; a merely UUID-looking unused name is allowed.
- Rename through a continuation changes the root authority only; exact historical transcript/message references and permanent IDs still resolve.
- A released slug ceases to select the former product; a new product may acquire it without redirecting pinned async writes.
- Forward migration preserves effective root names and stable historical evidence, while no normal reader or writer accesses a competing chain-name authority.
