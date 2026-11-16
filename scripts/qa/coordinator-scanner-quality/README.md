# Actual-host scanner quality qualification

This opt-in experiment tests the frozen 37 synthetic semantic cases through real
SQLite source preparation, durable attempt reservation, the strict production
Luna/Low inference adapter, and atomic host application. It does not contact
workers or admit a foreground conversation turn. It changes only disposable
fixture databases.

First compile the owning IDE test binary under the shared build-slot policy and
retain an immutable executable copy. Run its nonignored
`coordinator_scanner::quality::scanner_quality_all_real_host_fixtures` test first.
The wrapper deliberately requires that retained binary; it never builds or
selects another binary implicitly.

Prepare with normal resolved provider configuration, without completion calls:

```sh
PHOENIX_SCANNER_QUALITY_TEST_BINARY=/absolute/retained-test-binary \
PHOENIX_SCANNER_QUALITY_MODE=prepare \
PHOENIX_SCANNER_QUALITY_OUTPUT=/absolute/new-prepared-directory \
./dev.py qa coordinator-scanner-quality
```

For a paired prompt revision, preserve the original prepared databases and use
them as seeds for a fresh preparation directory:

```sh
PHOENIX_SCANNER_QUALITY_TEST_BINARY=/absolute/new-retained-test-binary \
PHOENIX_SCANNER_QUALITY_MODE=prepare \
PHOENIX_SCANNER_QUALITY_SEED_PREPARED=/absolute/original-prepared-directory \
PHOENIX_SCANNER_QUALITY_SEED_SHA256=exact_approved_original_manifest_hash \
PHOENIX_SCANNER_QUALITY_OUTPUT=/absolute/new-paired-prepared-directory \
./dev.py qa coordinator-scanner-quality
```

This verifies the original manifest, corpus, oracles, prepared records and source
database hashes, then copies the closed databases into the new directory. Seeds
must contain zero scanner attempts. The normal preparation API runs with the
revised prompt profile and must produce byte-identical `ScanInput`, preserving
source/candidate identities, aliases and clocks. Only the prompt, batch binding,
composed byte count and recorded provenance change. The new manifest records the
original seed manifest hash. Prior live results remain separate; no stored IDs,
attempts or frontiers are rewritten. Independent approval of the new complete
manifest is still required before another live invocation.

Preparation creates one real migrated file database per case. Source rows retain
the supplied role/origin class and prose. Every scanner-source message receives an
opaque UUID at initial admission. Logical corpus message names and their answer
categories stay only in the hash-bound prepared review record; they never enter
`ScanInput`. Validation matches the actual captured UUID through that host-only
alias map and rejects any logical source alias appearing in serialized inference
content. No persisted message identity is rewritten. Subscription-event sources first create
a real watched worker, authoritative materialized input, exact response and
terminal settlement. The resulting normalized event IDs replace only logical
fixture event aliases at initial admission; the hash-bound prepared record
retains those aliases, and actual prepared provenance is checked against them.
No persisted origin or identity is rewritten. Existing candidates are created/retired via
the native tracking mutation path, with real generated identities. Timestamp-only
synthetic fixture adjustments occur before the first scanner preparation to
preserve the source and retirement chronology. No IDs, origins, frontiers, batch
tokens or production limits are rewritten. There are no canned read results.

U01 stores roughly 1 KiB of unrelated prose, the full Unicode request and hold,
and 32 KiB of unrelated suffix. The normal 8 KiB encoded-source budget produces a
genuine partial fragment with a full-source digest and unread suffix. I01 creates
seventeen real items: matching Azure first, then sixteen unrelated bookmarks.
The ordinary max-sixteen candidate selector omits Azure without any exact source
overlap. Both fixtures assert actual prepared text/visible identities against
the frozen reference inputs. These input corrections leave all semantic oracles,
restriction requirements, denominators and acceptance thresholds unchanged.

Only the actual `ScanInput` from `prepare_coordinator_scan` is serialized into the
production bounded request. Oracle/withheld fields never enter inference. Host-only
full source/candidate seeds populate the disposable database, and only the real
bounded projection enters inference. All actual fragment IDs, offsets, roles,
origins, candidate IDs/omissions, request byte counts, source database hashes and
bindings are retained. The manifest freezes corpus/oracle hashes, production
prompt and DTO source hashes, exact executable, embedded compile-time source commit and preparation workspace
commit, actual outbound
model/connection, endpoint hash and limits. No account or credential is printed.

After independent review of that exact manifest and all revised inputs, an
explicitly approved live invocation uses a fresh output directory:

```sh
PHOENIX_SCANNER_QUALITY_TEST_BINARY=/absolute/same-retained-test-binary \
PHOENIX_SCANNER_QUALITY_MODE=live \
PHOENIX_SCANNER_QUALITY_PREPARED=/absolute/approved-prepared-directory \
PHOENIX_SCANNER_QUALITY_APPROVED_SHA256=exact_independently_approved_manifest_hash \
PHOENIX_SCANNER_QUALITY_OUTPUT=/absolute/new-live-directory \
./dev.py qa coordinator-scanner-quality
```

Live mode first checks an explicit independently approved SHA256 of the complete
prepared manifest, including source/alias-mapping record hashes. It checks the
exact protocol/binary/corpus/route and source database hashes,
reprepared binding and input hashes before any call. An exclusive marker permits
one invocation per prepared directory. Verified closed source databases are copied
to the fresh live case directories; inference/application never mutate the frozen
prepared databases. Limits are one call per case, at most37
calls, no retries, at most1 MiB aggregate logical system+serialized-input content,
30 seconds/request, accepted output at most2,048 reported tokens and16 KiB UTF8
text. Missing output usage, nontext, truncation or oversize fails the production
adapter. The Codex bridge does not enforce the requested generation/billing cap;
these are accepted-output, request-count, logical-input and timeout limits.
Unknown usage fields stay unknown. A timed-out/cancelled process may retain only
partial evidence; neither recorder exit nor partial results qualify promotion.

Records preserve original response blocks/stop/usage/replay and the raw proposed
JSON before application. Quote/source auditing includes proposals rejected by
the adapter or host. Every received proposal remains in the quality denominator;
a rejected capture cannot improve precision by disappearing. Host accepted
items/evidence/retirements/coverage are recorded independently before/after, as
are unchanged message and foreground-observation counts. Failures do not trigger
retries. Source-storage authority uncertainty stops the run without cleanup.

Independent review must adjudicate every frozen semantic gate in
`specs/coordinator-scanner/corpus-oracles.md`: ≥10/11 requested units, ≥4/5 bookmark
units, ≤1/11 visible semantic duplicates, exact source validity, zero false
commissions/restriction loss/reopen or authority violations, separate H01/I01 and
L01/L02 judgments. One proposal cannot satisfy two independent expected outcomes.
The recorder reports `recorded_not_semantically_qualified`; no keyword-only
rubric or successful test exit can establish model quality. Synthetic small-case
quality also does not replace host restart, generation, fairness, durability,
no-wake, continuation or foreground-action journey gates.
