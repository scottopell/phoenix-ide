# Current-source observation UI composition packet

## Authorities and boundary

- Immutable observation base: `f3893088b3cb224be9403f600173d5216e3a2f6b`.
- Historical product source: `9ee218d4a33ebd4b94ea547d60caa62134edb960`.
- Explicit private product/test exception: only upstream
  `e34ff8b85148bdb79f8c4164c5a7b067c930421a` applied as two-page hunks to
  `963307eb834fee0568fb8f991164d842d0ec68c2`. This is a tiny new runtime delta
  from 9ee plus its forced-window regression, not whole-UI product parity.
- Explicit private SQL child: the exact single-DB-file patch from reviewed product
  `250f3d56b13875caa3c927d453ecb70db406803f` to
  `f16afb227d9336a863c0b0345960ea9a6d62b7c7`, applied to attribution checkpoint
  `f9477018529420b462c896d83c115dbc318d8ef9`. Only `snapshot_ceiling AS MATERIALIZED`
  and the corrected upstream test/helpers are added. This is a real SQL exception,
  not observer-only SQL parity or a whole-product-file replacement.
- Worktree: `.phoenix/current-task-observation`, branch
  `bench/current-task-observation-9ee`. This packet is a source-only qualification composition.
- Read the [observation requirements](../specs/conversation_observation/requirements.md)
  and [operator guide](conversation-observation.md) before using this composition.

This is a source composition, not a complete UI-tree identity claim, built artifact,
fixture, browser capture, deployment or authorization to run a server. Backend,
API, immutable admission, capabilities, launch/root/shell, existing rig-support
scripts and generated observation types remain the actual f389 source except the
separately bounded [attribution exceptions](conversation-observation-attribution.md).
The e34 private backport preserves all 963 Rust bytes and all UI bytes except its
two exact approved page patches and the manifested source-parity guard. No old
branch, fixture or captured packet was imported. No fake init or schema-default
fields were added. No dependencies, migrations, lower hooks or runners were added.
The subsequent f16 child changes only the approved DB SQL/test hunks plus packet
source guards/docs/manifest metadata; observer/events/auth/configuration and all
other UI bytes are preserved from f947. The original e34 UI exception is unchanged.

## Exact parity inventory

[`conversation-observation-source-parity.json`](conversation-observation-source-parity.json)
contains a sorted union of every tracked UI path in both authorities, including
renderers, CSS, hooks, tool bodies, tests, assets/configuration and rig support:

- **859 files**: base, final-product and actual Git blob SHA-1 identities per path;
  null means absent in that authority, never an inferred identity.
- **824 files byte-identical to historical product**. Of **789 `ui/src` paths**,
  **774 are byte-identical** and **15 have explicit observation/test/type or
  approved e34 product/test exceptions**.
- Each of the **35 differences** has its exact historical-product-to-composed unified
  diff and a reason, not just a filename whitelist. Only temporary operand paths
  are replaced; patch bodies are not normalized. Twenty are unchanged f389
  rig-support scripts absent from the product authority; they are not renderers
  and were neither created nor run by this composition.
- MessageList, VirtualTranscript, ConversationNavStack and ConversationNav match
  the final-product blobs exactly. All other non-adapter UI source, including
  render/scroll/positioning/chapter/tool-body algorithms, also matches exactly.
- The new packet guard and this documentation are not product renderer files.

Final product fixes were restored as individual exact files or applied as bounded
App alias hunks, not as a whole-UI replacement. App's alias resolution now retains
the admitted journey across replacement admission and scopes errors/fallbacks by
selection; Coordinator errors are likewise selection-scoped. Final MessageList
source-tool positioning and navigation-readiness fixes and their regressions are
present. ProductConversationPage differs from historical 9ee by its existing f389
capability gates and the explicitly approved e34 Close notification callback.
ProductConversationPage.test.tsx differs only by the exact e34 forced layout-commit
regression and its test hook. The existing empty-tail test is unchanged. No b778
cursor/citation changes or other later product hunks are imported.

The source guard `ui/tests/observationSourceParity.test.ts` checks inventory
completeness, all three blob identities, every exact exception diff, the bounded
adapter list, unchanged generated/rig source and an empty non-UI tracked diff
against the immutable base outside exact attribution exceptions and packet metadata.
The attribution guard additionally pins the composition documentation and UI
manifest themselves, as well as its Python producer and TypeScript consumer.
Inside the 2702 UI tree, only the source-parity guard and the two exact e34
backport files are exceptions. Python `approved_backport` and TypeScript
`appliedBackport` each reconstruct both files by applying the exact immutable e34
patch to immutable 963 bytes in scratch directories, then require whole-file byte
equality. Reverting the patch, adding bytes or mutating its body is rejected.
Manifest regeneration alone cannot approve another page edit: the upstream/base
constants and two-path inventory are independently pinned by both guards. A changed
file cannot pass merely because its name is on the adapter list.

All exception patches use explicit Myers, disabled indent heuristic, three context
lines, zero inter-hunk context, unsuppressed blank context lines, disabled external
diff/textconv/color, full index hashes and fixed a/ and b/ prefixes. The tracked `regenerate_manifests` function in
`tests/devpy/test_observation_attribution_source.py` shares Python's exact diff
arguments; TypeScript's `exactDiff` pins identical arguments. See the attribution
packet for the complete command, finite manifest dependency order, hostile-config
regressions and exact source-only reproduction commands. Regenerating patch
metadata does not alter the original UI renderer/product source.

## Explicit existing capability exceptions

Reviewed adapter differences:

| Paths | Exact purpose |
|---|---|
| App; App Markdown-navigation test | Preserve capability bootstrap, persisted-only shell, disabled refresh owner and test capability response; propagate final alias implementation |
| ApplicationCapabilities; PersistedObservation and its test | Retain typed fail-closed discovery, scoped persisted routes and existing ownership regressions; expose unexpected request URLs and explicitly reject live input/continuation/action controls |
| ConversationProvider | Retain only `refreshEnabled` refresh-owner gate |
| Four generated observation types | Actual f389 generated bytes, unchanged; no manual codegen |
| ProductConversationPage | Existing gates on live authority/transcript, controls, retry/fallback/close owners and telemetry; persisted-only read-only notice; exact approved e34 callback reads ownedSnapshotRef at notification time |
| ProductConversationPage.test.tsx | Only exact approved e34 layout-commit/pre-passive-listener regression and test hook; original empty-tail test unchanged |
| ChainWorkIdentityBlock | Existing host-observation gate passes null to PR-status hook |
| conversationViewLifecycle; useConversationViewJourney | Existing telemetry admission/`ensure` gate only; lifecycle algorithms otherwise identical |

The parent authorized retaining all existing observation control/reporting gates,
provided every exception is explicitly listed. The last two rows are included in
that exception inventory: all three files remain byte-identical to f389. This
packet introduces no lower-hook rewrite and does not claim literal lifecycle-file
parity. Removing these gates would enable unsupported observations or reporters.

The first focused run exposed **3 failures / 177 passes** after exact-restoring
ChainWorkIdentityBlock: ordinary observation started
`/api/conversations/row-work/pr-status`. Reinstating its existing f389 capability
gate fixes the causal regression; the request deny-list was not relaxed. The
current manifest records the resulting precise exception.

## Validation actually executed

Using the already-cached UI dependencies, no installation/build or services:

Historical composition validation (before the e34 exception):

- Vitest, one worker, **410 passed across 10 files**:
  `tests/observationSourceParity.test.ts`, `PersistedObservation.test.tsx`,
  `App.alias.test.tsx`, `App.markdownNavigation.test.tsx`,
  `ConversationNavStack.readiness.test.tsx`, `MessageList.test.tsx`,
  `CoordinatorPage.test.tsx`, `ProductConversationPage.test.tsx`,
  `conversationViewLifecycle.test.ts`, `useConversationViewJourney.test.tsx`.
- `tsc -b --noEmit`: passed.
- ESLint with warnings denied on all changed TS/TSX and the new parity guard:
  passed.
- `git diff --check`: passed. Backend/API/Cargo/dev.py/generated/rig-script diff
  against f389 is empty.

The 14 persisted-observation ownership tests use real startup/providers/history
projection and a mocked ConversationNavStack. They assert no SSE/live owner and
allow only the scoped persisted reads/target discovery, rejecting telemetry,
activation, provider/resource/event requests, and reject text input, continuation
checkboxes and live action buttons. These are not evidence that all
nested widgets or browser interactions were exercised. The source guard proves
that composing final renderer code does not discard the retained capability gates;
it is not a substitute for runtime isolation or browser acceptance.

The passing product tests emitted React act/render-update warnings, and the
Markdown-navigation fixture logged `cacheDB.init is not a function`; those tests
still passed. These are recorded follow-up observations, not a claim of clean
browser execution and not grounds to change exact product source in this packet.

Not run: full `dev.py check`, Cargo/build/codegen, fixture creation/copy/migration,
server or browser launch, hosted dispatch, network/resource measurements. No
binary, fixture privacy, nested-widget isolation or browser acceptance is claimed.

## Private e34 backport evidence boundary

The new upstream regression forces one stream notification from the active
snapshot's layout commit while the old listener is still registered, before
passive-effect resubscription. It asserts unchanged registration count at that
instant, exactly one notification, a second snapshot read and an empty displayed
tail. That window distinguishes it from the unchanged original empty-tail test.
The callback reads the current owned snapshot and rejects a different owner.
This is source/unit evidence for that window, **not proof that the separately
measured 963 failure has been fixed or that its cause was this race**. Preserve
failed logs and the separate 2702 baseline; do not relabel them as backport passes.

See [the attribution packet](conversation-observation-attribution.md#private-e34-backport-validation)
for exact cached commands and results. Observation gates, SQL, backend sources,
completion events and dispatch are unchanged by this backport. No installed
artifact, service, browser, GET, SQL operation or hosted dispatch is qualified.

## Private f16 SQL child evidence boundary

The attribution manifest retains 2702 as its baseline and separately names the
immutable f947 child base, reviewed product 250 base and f16 upstream source.
Python and TypeScript reconstruct the entire approved DB bytes by applying only
that upstream patch to f947, independently of manifest regeneration. Python also
retains the original observer token proof and compares current unwrapped tokens
to the exact upstream-patched 2702 baseline; it does not erase SQL or new tests.
Negative mutations cover SQL whitespace, helper edits, keyword reversal and test
changes. An f947-relative finite changed-path proof preserves all other tracked
bytes. The historical UI manifest is regenerated unchanged by the existing
producer; no UI capability, renderer, test, generated type, rig or asset changes
are included except source-parity guard metadata/logic.

See [the f16 source packet](conversation-observation-attribution.md#private-f16-sql-child-source-only)
for exact identities, real Rust selectors, cached checks and their evidence
boundary. New/retained Rust selectors are authored but **UNRUN**; no build,
install, push/network, service, GET, browser, SQL execution, qualifier-worktree
operation, runtime qualification or measured benchmark benefit is claimed.
