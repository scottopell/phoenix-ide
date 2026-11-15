# ADR-245: Historical Native Locators Preserve Bytes Without Fabricating Trust

- **Status:** Accepted
- **Date:** 2026-11-09
- **Affects:** REQ-FED-021, REQ-FED-023

**Qualifies:** [ADR-243](243_federated-native-create-and-rename-use-destination-actions.md), only its historical source-instance backfill and retained-provenance assumptions. Its live native-action and destination-authority decisions remain accepted.

## Context

ADR-243 called for preserving existing native source-call locators and backfilling local instance identity onto complete historical creation and rename locators. Completeness of SQLite TEXT columns does not prove that their bytes are valid UTF-8 or usable as live typed identities. A migration that reads lossy strings, rewrites locators, or infers peer ownership from malformed historical values would destroy evidence or grant authority the old rows never established. SQLite CHECK constraints can enforce structural origin/provenance shapes but cannot validate UTF-8 bytes.

## Decision

Migration preserves each of the four existing source ProductConversation, transcript, assistant-message, and tool-use locator TEXT values byte-for-byte, including malformed UTF-8. It backfills the database's durable local instance identity only if all four values decode as UTF-8, remain nonblank after trimming, and contain no NUL. A creation locator additionally requires its source ProductConversation ID to parse. A non-native or provenance-absent creation does not gain native provenance.

If internal creation cannot qualify, it retains the original locator values under explicit `historical_internal` origin, with no source instance and no trust for a new creation request. Its already-stored agent-authored objective is processed literally, without re-expansion or reconstruction. A rename receipt that cannot qualify retains its locator values without source instance; it grants no peer authority, and any new request using its operation UUID conflicts rather than replaying it as an authenticated peer's receipt.

Live internal-native creations and qualified rename receipts retain strict complete peer-qualified provenance. Trusted application write constructors enforce UTF-8, nonblank-after-trim, NUL-free locator values (and parse the creation source ProductConversation ID), while SQLite CHECK constraints enforce the representable origin/provenance shapes. Historical unqualified records are distinct from trusted live provenance; the schema does not pretend that CHECK validates UTF-8.

## Options considered

- Lossy-decoding old TEXT and writing it back would alter the source evidence and create new locators.
- Backfilling on non-null or nonblank-only tests would assign local peer authority to malformed historical evidence.
- Rejecting every malformed historical row would prevent migration despite the ability to preserve and process untrusted evidence safely.
- Relying on SQLite CHECK for UTF-8 validity would claim an unenforceable constraint.

## Consequences

- Historical evidence is retained without manufacturing attribution; unqualified creation and rename cannot become trusted native retries.
- Trusted new writes require constructor validation in addition to SQL structural constraints.
- This narrows ADR-243's historical backfill assumptions only; authenticated live create/rename behavior, identity closure, and no-fallback routing do not change.

## References

- `specs/federation/requirements.md` — REQ-FED-021 and REQ-FED-023
- `specs/federation/federation.allium`
- [ADR-243](243_federated-native-create-and-rename-use-destination-actions.md)
