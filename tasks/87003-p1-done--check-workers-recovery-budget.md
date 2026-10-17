# Configurable check worker budget for safe recovery qualification

Global commissioned this bounded implementation under user-authorized safe recovery. Exact recovery base be1fbc52c394e299e48268c4a5079dc22851f5ac; host checkout and recovery pin must remain untouched. Default budget remains four.

Implement documented, validated PHOENIX_CHECK_WORKERS override. Requested 1 must actually reach existing Cargo/Rayon/Vitest/nextest/codegen/test-thread consumers; reject invalid settings explicitly. Audit lane concurrency and describe the real guarantee: do not claim global one-process cap if lanes can overlap. Fix only minimum supported budget semantics, no broad scheduler/framework or skipped checks.

Scope dev.py/check configuration and adjacent deterministic Python/command-construction tests/docs. No host guard, memory attribution, runtime/application source, history/disclosures or production mutation. No unnecessary local compiler. Avoid broad tests that implicitly build Rust.

Qualification: deterministic regression tests covering default/override/invalid input and every existing budget consumer, independent exact-source review, appropriate exact-head hosted CI. Focused PR with explicit reviewed patch identity and exact-be1 recovery qualification. If current-main integration requires another branch, preserve recovery candidate separately and explain safe pin to Global; never silently include wake migration126 or change host deployment. Global owns landing/admission/deployment; no routine reapproval needed for scoped implementation.

## Source delivery completed

Global confirmed PR #897 merged on main as `ea2b4772e523c057805f48152ec8c3615c02beb4`, parent `b6e0ecb744cce1b2dad3b767182d53002768d5b1`, tree `35b5cdb16ddf1ee8131ef0f132bed813243371ae`; parent/tree independently verified through GitHub REST. Integration CI run 38289036663 completed successfully. Independent exact-source review approved the recovery candidate; hosted exact-source Python qualification passed 38+38 tests.

The separately admitted recovery pin remains immutable: `a2be7bea4264219f425a72bf341516fde71ecbfd`, tree `6771b3fb5cf57666f1850dbf73bf241817d94047`, on exact base `be1fbc52c394e299e48268c4a5079dc22851f5ac`, excluding wake126, with `PHOENIX_CHECK_WORKERS=1`. This task-status follow-up is metadata only and is not a replacement recovery candidate. The original recovery branch/pin and merged source are preserved.

Done means bounded source delivery, review, and hosted qualification are complete. Host resource/memory preflight, host qualification, recovery admission execution, and activation/deployment remain separately owned by Global and the host executor; no activation or full recovery-tree Rust-build claim is made here.
