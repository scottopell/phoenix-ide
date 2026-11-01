# Configurable check worker budget for safe recovery qualification

Global commissioned this bounded implementation under user-authorized safe recovery. Exact recovery base be1fbc52c394e299e48268c4a5079dc22851f5ac; host checkout and recovery pin must remain untouched. Default budget remains four.

Implement documented, validated PHOENIX_CHECK_WORKERS override. Requested 1 must actually reach existing Cargo/Rayon/Vitest/nextest/codegen/test-thread consumers; reject invalid settings explicitly. Audit lane concurrency and describe the real guarantee: do not claim global one-process cap if lanes can overlap. Fix only minimum supported budget semantics, no broad scheduler/framework or skipped checks.

Scope dev.py/check configuration and adjacent deterministic Python/command-construction tests/docs. No host guard, memory attribution, runtime/application source, history/disclosures or production mutation. No unnecessary local compiler. Avoid broad tests that implicitly build Rust.

Qualification: deterministic regression tests covering default/override/invalid input and every existing budget consumer, independent exact-source review, appropriate exact-head hosted CI. Focused PR with explicit reviewed patch identity and exact-be1 recovery qualification. If current-main integration requires another branch, preserve recovery candidate separately and explain safe pin to Global; never silently include wake migration126 or change host deployment. Global owns landing/admission/deployment; no routine reapproval needed for scoped implementation.

## Source completion receipt

Source delivery completed through PR #897, merged as `ea2b4772e523c057805f48152ec8c3615c02beb4`, tree `35b5cdb16ddf1ee8131ef0f132bed813243371ae`. Integration CI run 38289036663 succeeded; independent exact-source review approved the recovery candidate and hosted exact-source Python qualification passed 38+38 tests. Evidence: https://github.com/scottopell/phoenix-ide/pull/897#issuecomment-6131028976.

The separately admitted recovery pin is preserved unchanged: `a2be7bea4264219f425a72bf341516fde71ecbfd`, tree `6771b3fb5cf57666f1850dbf73bf241817d94047`, on base `be1fbc52c394e299e48268c4a5079dc22851f5ac`, excluding wake126, with `PHOENIX_CHECK_WORKERS=1`. This metadata receipt does not replace or rebuild that candidate.

This current-main rename supersedes the stale metadata proposal #899, whose squash/add ancestry must not be blindly merged. Done denotes source delivery, review, and hosted qualification only. Host resource/memory preflight, host qualification, and activation/deployment remain separately owned; this receipt asserts none of those occurred and makes no full recovery-tree Rust-build claim. Global retains merge/admission/deployment authority.
