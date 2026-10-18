# ADR-129: One conversation name with an allocated route projection

- **Status:** Accepted
- **Date:** 2026-10-18
- **Affects:** REQ-CID-001–006, REQ-CHN-007, REQ-COMP-001/002 — `specs/conversation-identity/requirements.md`

## Context

ProductConversation already provides permanent identity across continuation transcripts. Root `title`, a chain-specific `chain_name` override, and slug-editing paths could disagree about the visible name. A readable current route must not turn renaming into an identity change, and duplicate human names must remain usable.

## Options considered

1. Add ProductConversation name/slug columns and retain the transcript naming authorities. This duplicates state and requires a larger schema transition.
2. Retain independently editable title, chain name, and slug. This leaves competing authorities and conflates human naming with route allocation.
3. Use root `title` as the only name and root `slug` as its allocated projection, retiring `chain_name`.

## Decision

Choose option 3. Fold each existing root's effective name into its title, drop the competing database column and domain field, and release all migrating ordinary root slugs, then reallocate projections in `(created_at, id)` order in a forward migration. Keep immutable product/transcript/message identities unchanged.

Use `BEGIN IMMEDIATE` for ordinary rename and creation publication, and allocate against all transcript slugs and actual product/transcript IDs. Derive a lowercase Unicode alphanumeric/hyphen base; use `conversation` for punctuation-only input and integer suffixes starting at 2 for collisions. Route producers percent-encode the UTF-8 slug as one path component.

Existing title, presentation, slug, and canonical-route wire fields carry the contract; no parallel product-name field is introduced. Compatibility endpoints delegate to the same title mutation. Clearing a legacy override resets the single name to `Conversation`; it cannot reveal an obsolete hidden name after the override authority is retired.

Permanent product UUIDs and exact evidence references remain stable. Former slugs are neither reserved nor retained as aliases. Async generators capture an immutable identity and the root name/projection baseline, then recheck existing admission and that baseline in the committing transaction. A stale generated result cannot overwrite a newer selected name. Continuation does not create a new product name.

## Consequences

- Duplicate human names are valid; URLs may have numeric suffixes.
- A copied readable route can cease to resolve or can refer to a later allocation after rename; durable citations use stable identities.
- Old route/API consumers remain accepted within their existing compatibility contracts, without preserving the removed database column or adding a redirect registry.
- Migration preserves effective root names and historical identities, not independently editable slug values or hidden overridden titles.
- The change is a backend/spec foundation for separately qualified UI integration; it is not a new persistence or authorization framework.

## References

- `specs/conversation-identity/requirements.md`; `specs/chains/requirements.md` REQ-CHN-007.
- `specs/compatibility/requirements.md` REQ-COMP-001/002 and ADR-034 govern forward-only compatibility; no old-slug registry or older-binary guarantee is added.
- Migration 129 is staged feature work, not an expansion of any exact migration-128 incident/deployment admission.
