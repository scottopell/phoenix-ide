# Configurable check worker budget for safe recovery qualification

Global commissioned this bounded implementation under user-authorized safe recovery. Exact recovery base be1fbc52c394e299e48268c4a5079dc22851f5ac; host checkout and recovery pin must remain untouched. Default budget remains four.

Implement documented, validated PHOENIX_CHECK_WORKERS override. Requested 1 must actually reach existing Cargo/Rayon/Vitest/nextest/codegen/test-thread consumers; reject invalid settings explicitly. Audit lane concurrency and describe the real guarantee: do not claim global one-process cap if lanes can overlap. Fix only minimum supported budget semantics, no broad scheduler/framework or skipped checks.

Scope dev.py/check configuration and adjacent deterministic Python/command-construction tests/docs. No host guard, memory attribution, runtime/application source, history/disclosures or production mutation. No unnecessary local compiler. Avoid broad tests that implicitly build Rust.

Qualification: deterministic regression tests covering default/override/invalid input and every existing budget consumer, independent exact-source review, appropriate exact-head hosted CI. Focused PR with explicit reviewed patch identity and exact-be1 recovery qualification. If current-main integration requires another branch, preserve recovery candidate separately and explain safe pin to Global; never silently include wake migration126 or change host deployment. Global owns landing/admission/deployment; no routine reapproval needed for scoped implementation.
