# Add ranked conversation-content search to the Cmd+P palette

## Observed journey

A user remembers discussion text such as “tmux pty test exhaustion” but not the conversation slug. Cmd+P currently supports `c <query>`, but that query only fuzzy-matches slugs held in the client-side conversation list. The user needs to type `c tmux pty test fix`, see unique conversations ranked by matching message content with a useful excerpt for each result, and open the selected conversation to continue unresolved work.

The requested command split is:

- `c <text>` — search full conversation content.
- `cs <slug>` — retain the existing client-side fuzzy conversation-slug search under a new prefix.

Content search must include active and archived user conversations, while excluding Coordinator/internal and sub-agent conversations. Archived results must be visibly identified. “Fuzzy” means SQLite FTS lexical relevance plus partial final-token prefix matching, not edit-distance/typo correction.

## Verified findings

- `specs/command-palette/requirements.md` REQ-CP-009 currently assigns `c ` to conversation-only fuzzy slug search and requires a non-whitespace `c...` query to remain global. The requested command split is therefore a normative behavior change, not just an API addition.
- `CommandPalette/stateMachine.ts` parses `^c\s(.*)$`; `CommandPalette.tsx` then limits eligible sources to `ConversationSource`. `ConversationSource.ts` fuzzy-matches only `Conversation.slug` from the active conversation list.
- The palette already has the needed async mechanics: a 120 ms debounce, `AbortController`, stale-result suppression, keyboard selection, and support for a `PaletteItem.snippet`. Code-search results already demonstrate snippet rendering.
- Phoenix already maintains one application-wide SQLite FTS5 index over typed, human-readable message text. `phoenix-db::Fts5Retriever` owns natural-language query construction, BM25 ranking, snippets, mutation hooks, hard-delete pruning, startup reconciliation/backfill, and freshness reporting (`specs/conversation-retrieval/`, `crates/phoenix-db/src/retrieval.rs`, migrations 18 and 51).
- The FTS table is a rebuildable derived cache over `messages`; the existing bundled SQLite build enables FTS5. A second FTS index or a new JSON representation is neither necessary nor permitted by the retrieval contract.
- `RetrievedChunk` already carries conversation/message provenance, a display snippet, and BM25 score. `RetrievalScope::Conversations` filters before `LIMIT`; broad retrieval consumers can also gate on `index_reconciled()`.
- Active and archived list APIs are separate. Active listing excludes archived, non-user-initiated, and Coordinator conversations. Archived listing selects user-initiated rows but does not explicitly apply the active list’s Coordinator exclusion. The content-search visibility policy must therefore be enforced in the backend query/scope rather than by filtering already-limited FTS hits in the UI.
- Command-palette parsing and behavior are covered by `stateMachine.test.ts` and `CommandPalette.test.tsx`; retrieval query construction, scope, maintenance, reconciliation, and deletion have Rust tests in `phoenix-db/src/retrieval.rs`.

## Owning invariants

1. Ranked recall of message content goes through the single `MessageRetriever` abstraction; the API and UI must not query `messages` directly or create a palette-specific relevance implementation.
2. Visibility filtering happens before the ranked result limit. Hidden/internal/sub-agent hits cannot consume the result budget and then be discarded afterward.
3. One palette row represents one conversation. Its preview comes from that conversation’s strongest matched message, while its ordering is deterministic and relevance-based.
4. Search results contain navigation/display data, not a parallel copy of conversation content. The durable source remains `messages`; `message_fts` remains a rebuildable derived cache.
5. Every async query is abortable and stale responses cannot replace results for a newer input or mode.

## Proposed implementation

### 1. Specify the command split and retrieval behavior

Update `specs/command-palette/requirements.md` and `executive.md` so that:

- `c` followed by whitespace selects conversation-content search and preserves the full raw input.
- `cs` followed by whitespace selects conversation-slug search and preserves today’s in-memory fuzzy-slug behavior, recent defaults, and selection behavior.
- Prefix parsing is longest-prefix-first; `cs ` cannot be interpreted as a global query or as `c ` content search.
- `c <query>` returns unique active or archived top-level/user-facing conversations, excludes Coordinator/internal and sub-agent conversations, displays the best matching excerpt, ranks by content relevance, and visibly marks archived rows.
- Content search requires non-empty text; `c ` presents a search-specific prompt/empty state rather than running an unbounded full-content query. `cs ` retains the existing recent-conversation defaults.
- Removing either prefix returns immediately to the normal global source set.
- Non-whitespace forms such as `code` and `css` remain unscoped global text, preserving the current explicit-whitespace rule.
- Index warming and backend failure are distinguishable from an authoritative no-match result.

Cross-reference `specs/conversation-retrieval/` for ranking, provenance, typed text extraction, maintenance, and freshness rather than duplicating those rules. Update the retrieval requirement only as needed to encode the selected user-visible scope and prefix-query policy in the shared primitive. Run the authoring pre-flight in `specs/AUTHORING.md` before pushing spec changes.

### 2. Extend the shared retrieval contract without adding another index

Evolve the typed retrieval request/scope so the caller can express “user-visible top-level conversations, active or archived” and lexical final-token-prefix matching without raw booleans or UI-side filtering. The backend scope must join/filter conversation metadata in the FTS query before ordering and limiting, including at least:

- user-facing/user-initiated conversation identity,
- exclusion of `runtime_role = 'coordinator'`,
- exclusion of child/sub-agent conversations,
- both `archived = 0` and `archived = 1`, while respecting any deletion-pending visibility rule already used by archived listing.

Keep ordinary natural-language retrieval behavior available to existing Chain QA and Coordinator callers. Represent the prefix-enabled query policy structurally (for example, a typed retrieval query/match mode) rather than changing unrelated callers implicitly.

For the prefix policy, normalize and stop-word-filter natural-language terms as the existing builder does, retain OR semantics, and add a safe FTS5 prefix expression for the final content-bearing token. All MATCH syntax must be generated internally from tokenized terms and values/limits/scopes must remain bound; users must not be able to inject FTS operators. General misspelling/edit-distance matching is explicitly out of scope.

Do not add an FTS migration or backfill path unless implementation evidence reveals a missing persisted capability. The existing `message_fts` + `message_fts_rows` schema and startup reconciliation are the intended substrate.

### 3. Add a bounded, typed HTTP search surface

Add a dedicated GET endpoint such as `/api/conversations/search?q=...&limit=...` backed by `AppState.message_retriever`. Define a typed response containing only the fields the palette consumes, for example:

- conversation id and slug,
- archived status,
- strongest matching message id/type/time as provenance where useful,
- sanitized/bounded FTS snippet,
- deterministic relevance score/order.

The endpoint must:

- trim and reject/short-circuit empty queries,
- clamp the result limit,
- report index-warming distinctly (use an intentional status/typed error contract),
- translate retrieval failures through the normal API error path,
- deduplicate message hits into one result per conversation,
- use the strongest hit as the preview,
- rank conversations deterministically from bounded retrieval evidence (primary content relevance, with documented match-count/recency/id tie-breakers), and
- fetch/join conversation metadata without N+1 queries.

Ensure aggregation does not allow one conversation’s many matching messages to starve all other conversations. Prefer grouping/ranking in the retrieval implementation before the final conversation limit; if a bounded over-fetch is used, specify and test its ranking limitations rather than presenting it as corpus-wide match counts.

Do not overload the existing conversation-list payload with snippets: search results are query-derived data for a different consumer and should have their own response type.

### 4. Split palette scopes and sources

In `CommandPalette/types.ts` and `stateMachine.ts`, model slug search and content search as distinct typed scopes. Parse `cs ` before `c `. Preserve raw input while passing only the suffix to the selected source.

Keep the current `ConversationSource` as the `cs` slug source (renaming its id/symbol if that makes ownership clear). Add an async conversation-content source that calls the new API, forwards the palette’s `AbortSignal`, maps each hit to a conversation palette item, and navigates to `/c/<slug>` on selection.

Update `CommandPalette.tsx` source routing so:

- `c ` invokes only content search,
- `cs ` invokes only slug search,
- global mode retains its current sources and must not begin expensive global transcript search implicitly,
- switching scopes immediately clears incompatible stale results,
- archived search hits remain navigable even though they are not present in the active `conversations` prop.

Use an explicit result kind/typed metadata for conversation-content hits rather than assuming every “Conversations” row embeds the existing `Conversation` object. This prevents archived API hits from being confused with active list records.

### 5. Render useful previews and state

Use the existing snippet presentation path or a conversation-owned variant to show the bounded best-match excerpt below the slug/title. Preserve compact information density and keyboard visibility. Add an inline archived indicator to archived results; do not hide it in hover-only UI.

Provide distinct UI states for:

- `c ` awaiting text,
- the debounced/in-flight search,
- index warming,
- authoritative no matches,
- request failure.

Keep the input focused, selection stable, and rows readable within the palette’s existing height. Colocate any new component-specific CSS with the CommandPalette owner when practical; do not expand `index.css` unless the style is genuinely shared.

## Acceptance and regression coverage

### Rust/database

- Natural-language terms still use safe OR matching and stop-word removal.
- A partial final token matches a longer indexed token; earlier complete terms retain lexical/stemmed matching.
- Quotes, punctuation, wildcard/operator characters, stop-word-only input, Unicode, and empty input cannot inject MATCH syntax or fail the query.
- User-visible scope includes active and archived top-level user conversations and excludes Coordinator, non-user/internal, sub-agent/child, hard-deleted, and deletion-pending conversations before limiting.
- Multiple matching messages produce one conversation result with the strongest relevant snippet.
- Several matches in one conversation do not starve a relevant second conversation from a bounded result set.
- Ranking and all tie-breakers are deterministic.
- Hidden messages remain excluded as in the existing retriever.
- Existing insert/update/delete/reconciliation tests continue proving that live changes appear without restart, edits replace stale text, and hard-deleted content cannot resurface.
- Endpoint tests cover limit clamping, empty query, warming, no match, archived metadata, scope exclusions, result shape, and backend failure.

### UI/state machine

- `c tmux pty test fix` parses as content scope and sends only `tmux pty test fix` to the API.
- `cs emo` retains fuzzy slug ranking and keyboard navigation without calling the content endpoint.
- `c ` and `cs ` clear results from the prior/global scope immediately.
- `code`, `css`, and `> c ...` retain existing global/action semantics.
- Out-of-order or aborted requests cannot display stale content results.
- A result displays its matching excerpt; archived results display an inline archived marker.
- Enter/click opens both active and archived results by slug and closes the palette.
- Warming, loading, no-results, and failure states are visibly distinct and accessible.

### User-journey validation

Seed at least two active and one archived user conversation with overlapping terms, plus excluded internal/sub-agent rows. Open Cmd+P and verify:

1. `c tmux pty test fix` shows unique eligible conversations ordered by relevance with matching excerpts.
2. A partial final term such as `c tmux pty exhaus` finds content containing “exhaustion”.
3. An archived match is labeled and opens successfully.
4. Excluded internal/sub-agent content never appears.
5. `cs <partial-slug>` behaves like today’s `c <partial-slug>`.
6. Rapidly replace one content query with another and verify no stale-result flash.

Run `./dev.py codegen` if the chosen response types participate in Rust→TypeScript generation, targeted Rust/UI tests during development, and `./dev.py check` before completion.

## Risks and non-goals

- **Ranking semantics:** BM25 scores are lower-is-better and are message-level today. Conversation aggregation must not accidentally invert scores or claim exact corpus-wide match counts when based on over-fetching.
- **Scope drift:** active-list and archived-list SQL are not identical. Centralize or test the search visibility predicate so Coordinator/sub-agent/deletion-pending rows cannot leak.
- **Archived navigation:** verify the existing `/c/:slug` loader can open an archived hit; fix only the minimum route/load behavior if search exposes an existing gap.
- **Index freshness:** startup reconciliation is asynchronous; an empty result while warming must not look authoritative.
- **No second search index, embeddings, network search, typo/edit-distance correction, transcript highlighting after navigation, implicit transcript search in unprefixed global mode, mobile palette expansion, or redesign of file/code search.**
