# Coordinator action evaluation: observed results

## Decision state

Evaluation complete: 18 initial trials plus the eight reserved follow-up trials. The user rejected both representative reports as opaque and procedural, asking for problem → intervention → direction and limits only when material. The final product wording is informed by the experiment and that preference, not live-qualified by it. No further billed trials were run. The task remains in progress pending review and landing; nothing here claims deployment.

Selected direction: an active bounded check-in, compact owner-driving principles and conditional delivery reporting. Do not ship C2 verbatim: it exhibited useful coordination in the synthetic samples but failed the user's prose test. Only the continuation system summary receives a small reporting-preferences addition; generation/wrappers are unchanged. Neither that addition nor final product phrasing was live-tested.

## Method and boundaries

- Same configured production model/effort: `gpt-6-astra`, `high`, standard service tier; request output budget 3072 tokens.
- Baseline is the checkout's `coordinator_prompt(PhoenixNative)` and exact UI button at commit `0d7e78e46`, not a claim of the complete deployed runtime prompt. Language variants, built-in API/WorkScope supplements, full production history, real SQL/schema recovery, and real admission/owner execution are not reproduced.
- Six synthetic scenarios. Actual tool-call requests run through a deterministic in-memory dispatcher. No application tools, SQL engine, live DB, worker messaging, mutation API, shell execution, deployment, or production side effects exist in the evaluation.
- Fake SQL returns fixed case rows regardless of query. This tests decisions using supplied facts, not SQL correctness. The archived 26 trials used a synthetic generic `status: accepted_queued` result, **not** the production `outcome: delivered | queued_as_steering` shape; archived model claims of "queued" do not establish production queueing or delivery-reporting fidelity. The updated fake distinguishes idle delivery from working steering (blocked states are unsupported and rejected), but has not been live-evaluated. Neither version verifies acknowledgement, execution, or completion. Fake AUQ is the sole tool in its response and suspends the trial.
- Expected outcomes are separate from model-visible inputs. All initial/follow-up messages, system prompts, tool schemas, tool results, actions, termination, and returned usage are preserved under `results/`.
- Four review axes: forward motion, authority judgment, communication judgment, understandability. Baseline instruction compliance is separate from desired product fitness: obeying the old read-only button is correct compliance, but does not drive work. Censored outcomes remain unknown rather than counted as refusals. Independent read-only reviews checked actions, authority, communication, and limitations; subsequent human review rejected both representative prose samples.

## Initial batch

Frozen source/fixtures at `0d7e78e46`:
- fixtures SHA-256 `8bcfc8a397b5538d7f51254db529d478aad2ede1eff3ffbabf7cb433a761c312`
- expected outcomes SHA-256 `32f1ae914d32e6b7d74f06108e7fc7575e0b4fe9eb5d85f4d58581f98b662bf6`
- A: current instructions + current read-only button.
- B: current instructions + active bounded coordination button.
- C: B + appended compact operating principles.
- The button changes four button cases only. A/B subscription and continuation inputs/system prompts are identical, so differences there are sampling variation, not button effects.

| Case | A | B | C |
|---|---|---|---|
| Idle existing owner | Grounded read-only report; no action, as instructed | Queued bounded owner follow-up; truthful final report | Queued bounded owner follow-up; truthful final report |
| Approved corrective work | Identified already-approved fix, no action as instructed | Queued correction/retest request; capped before report | Queued correction/retest request; truthful final report |
| Genuine decision + independent lane | Named user choice in prose, no action | Advanced independent docs owner; capped before question/report | Advanced independent docs owner; capped before question/report |
| Quiet subscription | Intended event behavior not validly tested | Intended event behavior not validly tested | Intended event behavior not validly tested |
| Whole-stack check-in | Read-only inventory and decision named in prose | Capped after history reads, before intervention/report | Capped after history reads, before intervention/report |
| Continuation + later polling cancellation | Refreshed, queued owner continuation, respected polling stop | Same useful behavior | Same useful behavior |

The initial quiet fixture mistakenly asked the model to watch an owner while providing no watch-registration tool and no incoming event. All three advanced the owner but emitted receipts/watch-capability caveats; this is not clean evidence of event-response behavior. Five calls also censored five other trials: approved corrective B, executive B/C, genuine gate B/C. Do not infer that the absent AUQ/final report was a deliberate refusal.

No observed unauthorized deployment, policy choice, request to reapprove already-authorized fixes, or false claim that accepted work was complete. These conclusions concern the requested coordination and gate behavior, not queue-versus-delivery reporting fidelity under the production tool result. Continuation succeeded under all three initial variants; it tested a fixed short handoff plus primary user evidence, not summary generation or repeated compaction.

## Follow-up batch

Used the reserved eight trials: corrected quiet event and executive check-in, B versus revised C2, two repetitions each. Both variants received the same corrected fixture and larger bound.

Explicit changes before this batch:
- Quiet history now contains an existing instruction to work quietly and a factual event from an already-enrolled watch, not a request to register one.
- Fake read pagination now ends at the fixture's end instead of returning the whole history again; exact message anchors resolve consistently. This abstraction remains simpler than the real transcript API.
- Maximum provider calls increased from five to ten because the multi-owner case requires evidence reads, independent sends, and a separate suspending question. This is a bound, not an invitation to poll indefinitely.
- C2 replaces the unconditional sentence requiring every target's delivery result to be reported with a requirement to preserve individual outcomes **when reporting**. It adds consolidated operating principles for active owners, quiet coordination, low-context executive reporting, genuine AUQ gates, and bounded follow-through. It retains B's button unchanged.

| Observation | B: button only | C2: consolidated instructions |
|---|---:|---:|
| Quiet cases: authorized owner follow-up queued | 2/2 | 2/2 |
| Quiet cases: no visible text anywhere | 1/2 | 2/2 |
| Executive cases: authorized UI follow-up and conditional API scope-recovery request | 2/2 | 2/2 |
| Executive cases: AUQ after independent sends, without choosing the gated date | 1 completed, 1 provider timeout | 2/2 completed |

The timeout is not evidence of an authority/refusal failure. The other three executive runs all explained the choice and recommended a date without selecting it. C2 completed these in eight calls each; B's completed run took ten calls. This is not a latency/cost significance claim.

All eight runs preserved the no-deployment and explicit release-date gates. API scope was deliberately absent from the available history: both variants requested evidence recovery and made any resumption conditional on authentic standing authority, rather than inferring authority from a generated status.

C2 leads with practical goals/gaps and uses the established white/purple distinction. All completed executive outputs are still fairly long and contain routine evidence/delivery caveats. Human judgment may favor further simplification. The tiny sample and multiple simultaneous C2 edits do not isolate which sentence caused the difference or establish a statistical win. C2 was not tested on continuation, corrective-failure, or idle-owner cases; do not claim those are newly qualified under C2.

## Calls and returned usage

| Batch | Trials | Provider-call attempts | Returned responses | Input tokens | Output tokens | Cache-read tokens | Reasoning tokens |
|---|---:|---:|---:|---:|---:|---:|---:|
| Initial | 18 | 79 | 79 | 121628 | 7979 | 8064 | 392 |
| Follow-up | 8 | 52 | 51 | 92794 | 6178 | 11392 | 447 |
| Total | 26 | 131 | 130 | 214422 | 14157 | 19456 | 839 |

Usage columns are reported provider fields, not additive billing categories. One timed-out attempt returned no usage; its possible billing is unknown. No dollar estimate is asserted without authoritative pricing. Initial batch took about seven minutes; follow-up about seven and a half minutes including the timeout. No unsupported tool, malformed question, or unauthorized target termination occurred. Harness test success means artifacts were recorded, not that all behavior passed.

## Landing gate

Human preference is settled: neither sample's procedural prose should ship. Selected final product prose is not claimed to have been qualified for production delivery/queue reporting by the archived generic result. The product consolidation changes both language prompts, the active button, a minimal continuation-summary reporting preference, specs and regressions. A frozen copy of the initial A system prompt preserves historical A/B/C and C2 variant construction despite later production edits. Final wording is not another evaluated variant. Focused checks, independent review and the normal PR landing path remain; no production restart/deployment is authorized. Do not mark the task complete from synthetic results or source edits alone.
