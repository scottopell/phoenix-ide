# R1 — Hidden GitRepository additive substrate and deterministic legacy backfill

## Problem statement

Phoenix's live repository facts remain carried by the legacy Project model, paths, and Project-shaped compatibility surfaces. Accepted ADR-032 permanently retires Project as a product/domain concept and defines hidden, opaque, Phoenix-local `GitRepository` identity attached singularly through nullable `WorkScope.repository`. Moving directly without a bounded expand phase would make migration correctness, historical conflicts, deployment compatibility, and rollback inseparable from the authority cutover.

R1 pays for one deliberately short-lived expand phase: add enough normalized hidden substrate and deterministic migration evidence to prove that legacy Project facts can be represented without ambiguity, while leaving **Project as the sole live repository authority**.

## Single goal

Land an additive, dormant, old-binary-safe relational substrate and deterministic legacy backfill that makes the later R2 atomic authority cutover reviewable and mechanically verifiable, without changing any production repository reader, writer, API, UI, or behavior.

## Delivery boundary

This task is the frozen implementation authority for R1 after its prerequisites merge. Creating and publishing this task does not start implementation. Once unblocked, R1 may implement only the additive expand/backfill/validation boundary below; it may not absorb R2 or any downstream milestone even when adjacent schema or types make that appear convenient.

## Requires

- PR #633 merged to `main`, providing the dormant truthful Close foundation and stable worktree identity prerequisite.
- Accepted ADR-032 and the merged GitRepository normative requirements/Allium as architectural and behavioral authority.
- Exact inventory of all Project-authoritative production readers/writers and all prospective GitRepository consumers before schema design is frozen.

## Provides

- Additive hidden `GitRepository` relational substrate with opaque Phoenix-local identity.
- Additive current-row substrate for Git common-directory and management-root locator observations, each structurally carrying exact `present`, `missing`, or `inaccessible` status and observation time when a truthful observation exists.
- Additive current-row substrate for an optional default-branch observation carrying branch identity when resolved, provenance (`remote_head_cache`, `local_checked_out_branch`, or `user_selected`), observation generation/time, or an explicit unresolved shape; never a fabricated `main`.
- Singular nullable dormant `WorkScope.repository` attachment.
- Deterministic migration-only Project→GitRepository backfill.
- Typed conflict detection and a read-only shadow-validation report/command suitable for proving R2 readiness without influencing production decisions.
- Documented expand/deploy/rollback posture for later R2.

## Forbids

- Any live GitRepository production reader or writer.
- Runtime Project→GitRepository synchronization, event mirroring, triggers, dual writes, or mixed authority.
- Repository authority cutover; R2 alone moves authority to `ProductConversation → WorkScope.repository → GitRepository`.
- Any user-visible Repository product, collection, identifier, title, grouping, lifecycle, task/branch/PR inventory, route, UI, or API contract.
- Exposing hidden GitRepository identity through product UI or client-facing payloads.
- Letting R1 rows influence existing Project path, worktree path, grouping, suggestion, usage, analytics, provisioning, or conversation payloads; all existing product surfaces continue to source legacy Project authority unchanged through R1.
- Project UI/API/storage removal or changed behavior.
- Close settlement/retirement orchestration, History finalization, permanent Delete, web ProductConversation presentation, or physical legacy deletion.
- Treating byte equality between a backfilled Project ID and GitRepository ID as runtime substitutability.
- Absorbing restart-repair adoption or downstream creation/observation behavior merely because adjacent schema/types are present.

## Authority before and after

| Point | Sole live repository authority | Dormant data |
|---|---|---|
| Before R1 | Project | None from R1 |
| After R1 | Project | Hidden GitRepository rows and nullable WorkScope attachments populated only by migration/backfill; observation rows exist only where retained source evidence is complete and exact; all are inspected only by read-only validation |
| After later R2 | `WorkScope.repository → GitRepository` | Project becomes read-only compatibility/retained legacy data until downstream deletion owner removes it |

R1 does **not** change authority. The only production path permitted to create the R1 rows is the deterministic schema migration/backfill running during database upgrade. No steady-state application writer is enabled.

## Merged-main invariant

After R1 merges, the same legacy Project facts drive every product decision and every writable repository fact as before. New GitRepository-shaped data cannot be consulted by a live product path, cannot alter a response or mutation, cannot be updated by normal runtime work, and cannot become a fallback authority. A missing, conflicting, or invalid shadow row is diagnostic evidence or a deployment-blocking migration failure—not permission to guess, self-heal through a runtime dual write, or fall back between authorities.

## Schema and backfill scope

The implementation PR may add only the relational structures needed for:

1. hidden repository identity;
2. one current observation per repository and locator kind;
3. one optional current default-branch observation per repository with total resolved/unresolved shape, provenance, generation, and observation time;
4. a nullable singular repository foreign key on WorkScope (or a normalized one-to-zero/one attachment whose relational constraints make singularity equivalent);
5. migration audit/conflict facts only if they are required to preserve a typed, queryable failure rather than encoding the collection in JSON; and
6. a read-only validation surface that compares legacy authority with the backfilled shadow without mutating either.

The implementation must use normalized columns/rows, foreign keys, `NOT NULL`, uniqueness, and `CHECK` constraints so malformed status/provenance combinations and multiple attachments are structurally unrepresentable. Do not freeze Rust function names, internal module layout, public HTTP shape, or a client-facing API in this task. SQL/table names may be selected during implementation review, but the semantic constraints above are fixed by the normative model rather than by an accidental API shape.

R1 does not add restart-repair evidence capture or Close adoption. If the merged #633 schema already retains such evidence, R1 may reference it only as read-only migration input when every required identity is exact; it may not add a live capture/adoption path. Any missing prerequisite is returned to its owner rather than absorbed by R1.

The backfill does not probe Git, the filesystem, remotes, branches, or clocks to manufacture observation history. Locator/default-branch tables may therefore remain empty for a repository whose legacy rows lack exact status/provenance/time evidence. Their absence is unresolved dormant shadow state, not a migration error and not permission to convert `projects.canonical_path` into `present` or legacy `projects.main_ref` into provenanced canonical-default evidence. Live observation capture belongs at or after the R2 cutover under one writer.

## Deterministic identity rules

- For every retained legacy Project row, create exactly one hidden GitRepository row by a replay-stable total mapping: `GitRepository.id = Project.id` byte-for-byte. This is a migration seed only and never runtime substitutability.
- Project ID is the complete R1 backfill partition. WorkScopes assigned to the same Project ID attach to that Project's one seeded GitRepository. WorkScopes assigned to different Project IDs always attach to distinct seeded repositories in R1—including when the records represent linked worktrees of one underlying checkout or separate clones, and even if paths, remotes, slugs, directory names, or commits match. Any cross-Project sameness anomaly is typed validation evidence for an owning repair/cutover decision, never an R1 merge or migration conflict.
- Re-running the migration against identical persisted legacy input must produce identical IDs, attachments, observation presence/absence, and conflict outcomes without random UUID generation, current filesystem/network reads, wall-clock-dependent values, path canonicalization as identity, remote-based identity, or row-order dependence.
- A backfilled locator or default-branch observation is created only when exactly one retained source row for that repository/observation kind already supplies the complete value/status or branch/provenance, its persisted observation generation, and its persisted observation time. The migration copies those values exactly. Zero complete rows produces no observation. More than one candidate at the same source identity/generation, or contradictory candidates, is fatal. R1 defines no source precedence or tie-break because it never chooses among candidates and reads no live signal. Incomplete legacy `canonical_path`/`main_ref` data creates no observation row.
- A path is a locator value, never the hidden identity. A reappearing path never proves continuity.
- R1 never substitutes source-row order or migration execution time for observation generation/time and never synthesizes historical precision the source did not retain.

## Conflict behavior

- If one WorkScope maps to more than one retained legacy Project assignment, migration fails transactionally with typed/bounded evidence identifying the scope and conflicting legacy identities. It never chooses first/latest/non-null/path-matching assignment.
- Duplicate or contradictory source rows that violate one-to-one Project→GitRepository replay stability are fatal to the backfill transaction. R1 defines no normalization or winner-selection escape hatch.
- Missing exact branch provenance/time produces no backfilled observation; it never fabricates a branch or assigns provenance by inspecting current Git state.
- Exact retained `missing` and `inaccessible` locator evidence remains distinct. A legacy path without retained status/time produces no locator observation and never becomes `present` by default.
- A conflict must leave the pre-R1 database authority intact and retryable after repair; no partial new-authority state may commit.

## Shadow-validation contract

The validator is a **typed sink** and read-only diagnostic, not a product reader or repair mechanism. It:

- reads legacy Project authority and dormant R1 rows in one read-only SQLite snapshot/transaction on a query-only connection, so mutation is structurally unavailable and every category describes one database state;
- checks total Project→GitRepository coverage, replay-stable identity mapping, one nullable repository per WorkScope, observation shape/provenance, FK integrity, and absence of conflicting assignments;
- emits bounded counts plus stable typed mismatch categories and enough identifiers for operator repair;
- exits/fails distinctly when any mismatch exists;
- performs no writes, repairs, authority selection, fallback, UI/API projection, branch/network observation, or runtime synchronization; and
- is usable before R2 to gate deployment/cutover and after an R1 rollback/forward retry to prove state.

The validator's CLI or internal entry-point spelling is implementation detail; its semantic output categories and non-mutating behavior are acceptance contracts. Tests must prove read-only behavior, including mismatch cases.

## R1 readiness evidence handoff

R1 produces one canonical **R1 readiness evidence bundle** for each candidate R1 implementation and target database. The bundle is one semantic aggregate: its members cannot be selected from different candidate R1 implementation heads, candidate schema states, target database snapshots, or validator runs. The old-binary exercise intentionally names one different, pinned preceding binary as its test subject; its result must be produced against this bundle's candidate R1 expanded schema and cannot be borrowed from another candidate. A later run supersedes an earlier bundle as a whole rather than allowing evidence to be mixed across runs.

The bundle binds:

- the exact R1 implementation identity and the exact schema/migration identity it expects;
- the target database identity, one validator run identity, and the single query-only SQLite transaction/snapshot boundary from which every database-derived result was read;
- the validator's typed overall result plus every typed category and bounded count, distinguishing valid absence from mismatch or incomplete/conflicting evidence;
- the production repository-authority census revision evaluated against that exact R1 implementation, including the conclusion that Project remains the sole live authority and that no R1 production reader, writer, fallback, repair, or dual-write path exists;
- the old-binary compatibility exercise result, identifying the exact preceding binary test subject and the candidate R1 expanded schema against which its representative Project-authoritative journeys passed; and
- the applicable binary-rollback posture, including that additive R1 schema is retained and destructive down-migration is prohibited.

A bundle is complete only when all members are evaluated for that same candidate R1 implementation/schema and target readiness run—with the exact preceding binary named solely as the compatibility test subject—the validator result is clean under this task's semantics, the census has no authority leak, the compatibility exercise passes against the candidate expanded schema, and the rollback posture remains applicable. Valid absence is recorded as typed/countable evidence and does not make the bundle unclean. The bundle records evidence only: it performs no live probe, repair, synchronization, authority selection, or cutover and introduces no required serialization format, path, CI provider, or storage product.

R2 consumes the complete bundle as its R1 handoff input, but the bundle does not authorize activation. R2 owns final post-quiescence revalidation against the then-current target database, superseded-writer exclusion, and the atomic authority activation gate.

## Deployment, old-binary, and rollback posture

R1 is an expand-only database change:

1. deploy a binary that understands the additive schema and runs the deterministic migration;
2. keep all live readers/writers on Project authority;
3. run read-only shadow validation and retain evidence;
4. if validation fails, block R2 and repair legacy source data through an explicitly owned path—never by enabling a second runtime writer;
5. allow rollback to the immediately preceding #633-era binary while Project remains authoritative; and
6. retain additive R1 tables/columns across binary rollback so old binaries ignore them and a forward redeploy can deterministically validate/reuse them.

R1 must not add a new required column to a table written by the old binary unless it has an old-binary-compatible database default and cannot alter old behavior. It must not rename/drop/reinterpret Project columns, install triggers that reject or mirror old-binary Project writes, or make old readers depend on new rows. Migration rollback is **binary rollback, not destructive down-migration**: do not drop shadow data while an old binary runs. R2 must explicitly exclude old binaries/stale workers before enabling GitRepository authority.

The old-binary compatibility exercise authorizes coexistence only while Project remains the sole live authority in R1. It does not authorize an old binary or worker to survive the R2 writer cutover; R2 must prove their exclusion before enabling any GitRepository writer or reader.

### Rent condition

R1 must be followed promptly by R2. Its dormant duplicate representation is temporary migration rent, not an acceptable steady state. If implementation review shows that R1 cannot remain **strictly additive, dormant, non-dual-writing, and old-binary-safe**, stop R1 and return the conflict to the coordinator/R2 owner. R1 must not absorb cutover. The delivery plan must then be replaced by one combined R1+R2 cutover PR authority with separately reviewable expand/backfill and cutover commits before implementation continues.

## Acceptance evidence

- [ ] PR #633 is merged and the R1 implementation branch is based on the merge containing ADR-032 and the GitRepository normative specs.
- [ ] Schema inspection proves additive normalized structures, relational singularity, total observation shapes, and no Project mutation/removal.
- [ ] Migration tests prove deterministic replay, exact Project-ID seeding, same-Project WorkScope convergence, distinct-Project non-collapse, missing observation evidence, and idempotent forward retry.
- [ ] Conflict tests prove one-scope/multiple-Project assignments and malformed duplicate source facts fail transactionally with no partial commit.
- [ ] Tests prove no production caller reads or writes hidden repository facts and no runtime path dual-writes Project/GitRepository.
- [ ] A production reader/writer census names every Project authority path and confirms none moved in R1; the census becomes the R2 cutover checklist.
- [ ] Shadow validation passes on valid upgraded fixtures and emits each stable mismatch category on adversarial fixtures without changing any row.
- [ ] One complete R1 readiness evidence bundle binds the exact implementation/schema, target database snapshot and validator transaction/run, typed result/categories/counts, production authority census, old-binary compatibility result, and rollback posture without mixing evidence across runs.
- [ ] Old-binary compatibility is exercised: upgrade with R1, run the immediately preceding #633-era binary against the expanded database on representative read/write journeys, then redeploy R1 and obtain the same validation result.
- [ ] Binary rollback documentation explicitly retains additive schema and blocks destructive down-migration.
- [ ] No UI/API snapshots, generated client types, or user-visible routes expose Repository or hidden identity.
- [ ] Existing Project/path/grouping/suggestion/usage/analytics/provisioning/conversation payloads are byte-for-byte behaviorally unchanged and remain sourced only from legacy authority.
- [ ] Applicable migration/spec tests, task validation, `allium check`, spec preflight when specs change, and `./dev.py check --all` pass.
- [ ] Exact-head adversarial review finds no single-authority violation, Projects-2.0 leakage, false client API lock, migration ambiguity, or downstream-scope absorption.

## Explicit non-goals and later owners

| Non-goal | Owner |
|---|---|
| Atomic reader/writer cutover to `WorkScope.repository → GitRepository`, exclusion of old writers, and compatibility direction reversal | R2 — repository authority cutover |
| Live Close settlement, WorkScope retirement, resource-admission fencing, destructive retirement, retry/recovery | L1 / task 92033 |
| Close outcome message, History transition/projection, permanent Delete, lifecycle-aware cleanup | L2 / task 92032 |
| Open/History aggregate web presentation and Project-surface retirement | P1 / task 92013 |
| iOS migration to stable aggregate contract | iOS vNext after L2 |
| Physical legacy Project storage/type/endpoint deletion and conceptual 1.0 QA | C1 / task 92016 |
| Browser transcript-cache follow-up | task 98005 after P1 |
| Changes to dormant Close foundation or stable worktree identity | PR #633 owner before merge; a separate prerequisite after merge |

## Feedback routing

| Finding class | Classification | Route |
|---|---|---|
| Additive schema cannot represent required hidden identity/observations/singular attachment | `FIX-NOW` | R1 |
| Backfill is nondeterministic, partial, heuristic, or conflict-silent | `FIX-NOW` | R1 |
| #633 lacks a required stable worktree identity/foundation fact | `PREREQUISITE` | #633 owner; do not patch around it in R1 |
| Needs live GitRepository reader/writer, Project writer exclusion, compatibility reversal, or authority selection | `DEFER:R2` | R2 cutover owner |
| Needs Close settlement/retirement or repair-evidence adoption | `DEFER:92033` | L1 |
| Needs History/finalization/Delete | `DEFER:92032` | L2 |
| Needs web presentation/Project surface retirement | `DEFER:92013` | P1 |
| Needs physical legacy deletion/conceptual QA | `DEFER:92016` | C1 |
| Unrelated repository/tool/runtime defect | `UNRELATED:<workstream>` | Return to owning workstream; record without expanding R1 |

## Dependency and status

Blocked on merge of PR #633. Do not start implementation while #633 is open or while its exact merged authorities are unavailable on `main`.

## Adversarial review ledger

The frozen brief was read specifically to falsify its migration boundary:

| Attack | Finding | Closure in brief |
|---|---|---|
| Single-authority violation | A migration-populated duplicate or a “current observation” could become a second writable/fallback authority. | Restricted row creation to upgrade migration; prohibited steady-state observation capture, writers, triggers, fallback, runtime repair, and all live reads; Project remains sole authority. |
| Projects 2.0 leakage | Hidden identity could accrete title/grouping/lifecycle/branch/PR ownership, become a collection API, or indirectly feed path-bearing compatibility payloads. | Explicitly forbids every new product surface/identity exposure and any R1 influence on existing Project/path/grouping/suggestion/usage/analytics/provisioning/conversation payloads; ProductConversation context has only the WorkScope attachment path after R2. |
| False API lock | Freezing a public validator endpoint or table/module spelling would constrain R2 without user value. | Locks only normative observation semantics and validation categories/read-only contract; leaves CLI/internal entry point, Rust modules, SQL names, and client API unstated; forbids a public Repository API. |
| Migration ambiguity | Linked worktree/separate-clone topology, multiple Project rows, duplicate source rows, observation precedence/timestamps, missing branches, and conflicting scope assignments permitted heuristic choices. | Defines Project ID as the complete R1 partition even for linked worktrees/separate clones; copies an observation only from exactly one complete retained source row including persisted generation/time; performs no live probes or tie-break; leaves incomplete state absent; and makes duplicate/contradictory candidates transaction-fatal. |
| Old-binary rollback gap | Additive tables can still break old writes through constraints/triggers or required columns, and R1 coexistence could be misread as permission past cutover. | Forbids old-table reinterpretation and mirroring/rejection triggers; requires compatible defaults, binary rollback test, retained expansion, no destructive down-migration, and explicit old-writer exclusion before R2. |
| R1 becomes permanent parallel representation | A dormant phase can linger and accrue readers, while a combined-PR escape hatch could let R1 absorb R2. | Requires prompt R2; if strict dormancy/old-binary safety fails, R1 stops and returns authority to the coordinator/R2 owner for a replacement combined R1+R2 cutover authority with separate commits. |
| R1→R2 handoff ambiguity | Individually valid validator, census, compatibility, and rollback artifacts could come from different heads, schema states, database snapshots, or runs. | Defines one non-composable readiness bundle tied to an exact implementation/schema and one target validator transaction/run; valid absence remains explicit, and R2 still owns post-quiescence revalidation and activation. |
| Downstream scope absorption | Restart evidence, Close adoption, UI removal, or Delete could enter because nearby types exist. | Makes each a named non-goal with owner and routing classification. |

No open adversarial finding remains in the brief. Implementation review may discover a #633 prerequisite; such a finding must be returned upstream rather than weakened locally.
