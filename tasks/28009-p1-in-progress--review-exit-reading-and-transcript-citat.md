# Preserve reading position across review exit and resolve exact transcript citations

Primary user commissioned mobile Global review→send/discard/back top-jump fix and bounded shared-overlay audit. Separate reported Proposal transcript/message citation reloads original Global: retain distinction from plain canonical-link pass.

Scope: viewer-only URL mutations must not replay reader navigation; preserve real route/source/hash navigation, state and finalization/history invariants. Exact typed transcript href safe normalization reaches existing pinned resolver. No cache/reinstall assumptions or generic scroll compensation.

Acceptance: failing-before routed-command and citation regressions, hosted actual reviewer flow geometry for ordinary/Global middle/bottom mobile/desktop, independent review and CI. No deploy/merge/device runtime authority. Shared goals/history owners confirm no conflicting hunks.

## Candidate evidence and bounded audit

PR964. Hosted39471159966 candidate0f34a73c5:30 reviewer journeys pass with zero mismatches/errors; old page/provider causal control reproduces30 top jumps (18 middle readers and12 pinned readers) and224 total navigation/geometry mismatches, no harness errors. Artifact https://github.com/scottopell/phoenix-ide/actions/runs/39471159966/artifacts/12288090449 . Cases cover ordinary+Global,390/1100/1440px, middle/pinned-bottom, send-to-draft, discard, close, Escape and fullscreen return. Middle anchor retained; bottom follows composer-height changes rather than imposing fixed scrollTop. Historical source swapping is removed from permanent CI after this receipt.

Root cause: provider-owned viewer replacements changed router navigation identity and dropped hash/state, so ProductConversationPage reissued completed reader commands. Commit-only navigation authority also prevents interrupted renders from consuming reader state.343 focused renderer/provider/page/finalization tests pass;32 alias integration tests pass. Exact reported Proposal target exists as an agent message in stored history; persisted Markdown literal was verified. Unsupported @transcript href previously became empty native _blank; safe UUID source-pinned promotion fixes that frontend boundary without changing backend admission.

Shared viewer mutation paths audited for prose/diff/browser/inspector; browser evidence is message review only. Outside-click close unsupported. Raw legacy ConversationPage1100px takeover remount remains an adjacent audit finding, not a cause established for current aggregate Global page; do not claim it fixed. Native keyboard/physical installed-PWA and deployed candidate remain unqualified. No repeat user repro requested, no local browser/build/deploy.

## Opaque-reference browser successor — transport gate remains open

The post-qualification review on approved head44c69263 was valid: UUID-only filtering contradicted opaque transcript/message identities. Original source approval was not applied to the successor. No merge, deployment or artifact dispatch occurred.

The successor moves typed citation promotion before Markdown URI normalization and uses an exact query entry through existing ordinary/Global admission. It preserves immutable aggregate ownership, pin membership, return state and message identity. Independent adversarial review reproduced and drove repairs for pending chapter selection, alias remount/Back/Forward, raw Global `%2F` decoding, and encoded static entry classification. Final browser-source review found no new actionable finding, conditional on transport and CI gates.

Final focused run: 533/533 tests in six suites; `PHOENIX_CHECK_WORKERS=1 ./dev.py check --lanes tsc` passed, focused App/alias ESLint and diff whitespace passed. Earlier 571/524 counts cover different overlapping selections, not additive totals. Tests retain fixture/cache/React warnings. No new physical/browser or Rust/full-check result is claimed. Retained evidence: `.phoenix/opaque-review/successor/`; original30 old-code and42 corrected-scroll journeys remain retained with their own immutable identities, not relabelled as successor runs.

Concrete remaining blocker: actual `api.resolveCoordinatorRoute`, `getConversation`, `getProductConversationSnapshot`, `getCoordinatorHistory` and `getProductConversationHistorySnapshot` transport exact IDs in HTTP path segments. WHATWG request normalization strips exact `.` and `..`, before server admission. Real-method fetch interception confirmed this; 65 ordinary opaque method/ID combinations survived request construction, which is not server/gateway acceptance. Fixing this requires lossless HTTP identity transport and matching read handlers/tests; percent-encoding dots is insufficient. Do not reject valid IDs, weaken exact membership, or treat mocked routing success as resolution of this gate.
