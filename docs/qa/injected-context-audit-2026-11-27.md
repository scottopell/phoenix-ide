# Phoenix production injected-context audit

**2026-11-27 · Full retained corpus · Audit source 5925130cd**

Coordinator batching and shorter sub-agent fan-in are the two candidates with modeled savings above 1% of recorded input. Continuation handoffs dominate long-lived injected text. The data supports measuring their retrieval and recall tradeoffs before changing them.

The consistent read-only scan covered 12,691 transcripts, 1,211,263 model-visible rows and 561,193 responses with positive recorded input. The response rows account for **60.205B input tokens**, including 51.010B cache reads. The separate usage ledger recorded **60.757B**, including 51.456B cache reads, across 566,077 accounting rows. Savings below use the ledger denominator. Carry share uses estimated message tokens multiplied by the estimated number of later recorded requests that carried the message. The cache-weighted share uses the brief’s 1.25× first carry and 0.10× repeats as a comparison model.

## 1. Top categories

| Category | Token-turns | Carry share | Cache-weighted units | Weighted share |
|---|---:|---:|---:|---:|
| Tool result, upper bound | 72.719B | 86.29% | 8.289B | 86.74% |
| Assistant | 9.675B | 11.48% | 1.064B | 11.13% |
| User typed | 661.8M | 0.79% | 70.0M | 0.73% |
| Continuation | 519.7M | 0.62% | 57.0M | 0.60% |
| Cross-conversation | 388.5M | 0.46% | 41.8M | 0.44% |
| **All injected, including the two above** | **1.215B** | **1.44%** | **133.2M** | **1.39%** |

The injected estimate is also about **2.00% of recorded input**. The carry model overstates the tool-result denominator because it treats all original tool text as retained.

## 2. Coordinator versus ordinary transcripts

Coordinator injected content accounts for **4.27%** of estimated carry and **4.27%** after weighting. Continuations contribute 2.11% of Coordinator carry and subscription events contribute 2.04%. For ordinary and sub-agent transcripts, the corresponding totals are **1.20% raw** and **1.17% weighted**. Cross-conversation input and continuations each contribute about 0.5% of that group’s carry.

## 3. Top ten individual injected messages

All ten are continuation handoffs at sequence 1. No skill or watch event reaches the top ten.

| Transcript | Estimated tokens | Carries | Token-turns |
|---|---:|---:|---:|
| 9bc3b72d | 10,220 | 867 | 8.86M |
| 61591d1d | 11,016 | 755 | 8.32M |
| 82e73b48 | 10,961 | 723 | 7.92M |
| 24ce75b7 | 8,704 | 862 | 7.50M |
| 67952f7b | 10,176 | 735 | 7.48M |
| dcba54e9 | 7,811 | 925 | 7.23M |
| 8521dc04 | 9,290 | 713 | 6.62M |
| cd59fa81 | 11,416 | 446 | 5.09M |
| e1033f6f | 10,419 | 436 | 4.54M |
| 63a7cdd3 | 9,330 | 482 | 4.50M |

## 4. Duplicate skills

Exact duplicate bodies among the 193 user-invoked skill messages account for **76,620 token-turns**, or **0.000091% of all carry**. The repeated skills that account for the measured cost are `phoenix-release` (50,193) and `prs-for-busy-folks` (26,427). A supplemental scan of the separate skill tool found 391 exact repeats among 3,872 successful calls, with 7.12M estimated token-turns before clearing (up to 0.012% of recorded input). The largest tool-repeat contributors were `phoenix-development` and `phoenix-api`. Almost all Coordinator repeats precede the final clearing watermark. About 4.94M come from ordinary tool calls or repeats after that watermark.

## 5. Superseded and stale events

Earlier local events with a later event for the same watch, source transcript and terminal outcome account for **116.1M token-turns** after the later delivery, or **0.138% of all carry** and **0.191% of recorded input**. Each occurrence may carry distinct evidence. This screens candidates for collapse and gives a ceiling for this candidate set. The stale-question marker occurred zero times among persisted subscription messages. The corpus contains 48 accepted awaiting-user-response watch events. Prioritize batching, and reconsider collapsing only where the event contract establishes supersession.

## 6. Follow-up approval plan copies

Six follow-up approval messages carry **3.91M plan-related token-turns**, or **0.0046% of all carry** and **0.0064% of recorded input**. Replacing those bytes with an illustrative 100-token pointer saves **3.69M**, or **0.0061% of recorded input**, before any retrieval. A separate prefix-based check found 33 initial approval messages with about 10.3M plan-related token-turns at the global calibration.

## 7. Wakes and batching

| Last trigger inferred from message order | Wakes | Input on first response |
|---|---:|---:|
| Cross-conversation | 17,715 | 2.403B |
| System other | 11,464 | 546.5M |
| Subscription | 9,271 | 1.296B |
| Continuation | 733 | 28.8M |
| Scheduled reminder | 160 | 16.1M |
| Sub-agent summary | 12 | 1.58M |
| Follow-up approval | 6 | 0.86M |
| Close failure | 5 | 0.59M |

There were **5,598** adjacent subscription-event pairs within 60 seconds. **5,039** had a recorded request between them. Counting just the first request for each of the **4,946** pairs without another input gives **695.1M tokens, or 1.14%** of recorded input, if those requests can be avoided. Counting all intervening requests gives a **1.330B, or 2.19%, ceiling** for that proxy. Both scenarios can overstate savings when a merged wake still needs the same work. The attribution uses the last eligible message, so generated observations, continuation handoffs and fan-in can be counted even when they are part of existing work.

## 8. Continuation pressure

Among **1,368** continued transcripts with recorded usage, injected text was **2.9% of final recorded input at the median**, **11.6% at p90**, and **4.5% on average**. The median was about 3,315 injected tokens. This share suggests injected content is usually a secondary factor, with more pressure in some transcripts. The final recorded request is a proxy for the input at the continuation decision.

## 9. Cache behavior

Recorded cache-write share was **0% in both groups**. The ledger marked the write bucket observed on 185,257 rows, so this field gives limited evidence about cache creation. Cache-read share was **35.3%** on the first request after a ≥4 KiB injection (1,252 requests), **84.8%** for all other requests and **82.7%** for other requests in the same set of transcripts. Among requests with a prior response within 60 seconds in transcripts with large injections, the comparison was **73.2% versus 90.2%**. These gaps are descriptive. Cold starts, handoffs and request timing affect the comparison.

## 10. Calibration

There were **11,577 usable fits** out of 12,379 transcripts with four or more measured requests. The median **0.255 tokens per byte** is roughly one token per four bytes and agrees with Phoenix’s rough text estimator for ASCII. The p10–p90 spread is **0.097–0.283**. Median fitted overhead is **18,665 tokens**, an intercept that also absorbs omitted or changing context. Two intercepts were negative. The lowest fit was **0.050** for 9779cd52 (425 requests, fitted overhead 83,967). The highest was **0.581** for 1256f0bb (21 requests, fitted overhead 3,830). Provider request metrics report OpenAI for the instrumented calls, and current OpenAI adapters split cached tokens from fresh input. Stored transcript model is a proxy for older and switched routes.

## 11. Unclassified data

`system_other` accounts for **123.7M token-turns (0.147% of all carry)** across 11,773 messages, with p95 **592 tokens**. A targeted source-matched scan identifies Coordinator registry observations (261 messages, about 18.3M token-turns at the global fit), initial approval plan copies, and sub-agent terminal guidance. The largest identified residual consists of 6,803 opening rows in sub-agent transcripts, consistent with task bootstrap input and worth tagging as such (about 34.4M token-turns at the global fit). Tag registry observations using their structured identity and classify bootstrap rows using runtime role and opening position.

`user_typed` p95 is **504 tokens**, far below the 5,000-token flag. **14,134 of 16,076** rows in that bucket have unknown historical provenance. Some automated input may be hidden there. Use the structured tracking and handoff identities and known typed runtime roles when tagging these records. Keep ambiguous historical rows explicitly labeled.

## 12. Ranked investments

| Candidate | Share of recorded input | Judgment |
|---|---:|---|
| Shorten sub-agent fan-in (fix 4) | **1.30%** at 1,024 tokens. **0.89–1.69%** at 2,048–512 tokens. | Worth a prototype with structured summaries and retrieval of full results. 7,549 summaries carried 1.469B estimated token-turns, with 12 standalone injected summaries. Most cost comes from the tool carrier. The estimate excludes the extra retrieval a change might cause. |
| Batch Coordinator subscription wakes (fix 3) | **1.14%** first-request scenario, **2.19%** all-intervening ceiling | Validate with a bounded delivery experiment that preserves distinct events and excludes mandatory Close and time-sensitive input. |
| Compact continuation handoffs | **0.79%** with a 4,096-token payload cap under the global fit | Largest injected source. Test recall and recovery before shortening. The cap estimate is sensitive to calibration and excludes retrieval. |
| Collapse eligible Coordinator events (fix 3) | Up to **0.19%** in the screened set | Defer. Prove supersession first and account for overlap with batching. |
| Replace follow-up plan copies (fix 2) | **0.0061%** at a 100-token pointer | Skip as a standalone token-saving change. |
| Deduplicate injected skill bodies (fix 1) | **0.00013%** | Skip for user-invoked skills. Skip a dedicated tool-skill project too, with a measured upper bound of 0.012% in that separate carrier. |
| Cap mandatory Close text (fix 4) | **0%** at 512 tokens | Skip. All six observed messages fall below that cap. |

The handoff scenario uses the global slope, which estimates **1.48%** gross carried input for all handoffs. The per-transcript fits estimate **0.86%** gross. This spread limits confidence in the cap estimate.

Cross-conversation traffic is another lead. It accounts for **0.64%** of input in carried message text and **3.96%** in attributed first requests. The audit does not establish which of those wakes are deferrable, so there is no savings estimate to rank.

## Method and limits

- Per-message token estimates are roughly ±25% as a working heuristic. Calibration outliers can exceed that. Images use 1,500 tokens each. The tool-result carry is an upper bound because the database stores only the latest clearing watermark, so historical clearing cannot be reconstructed. The estimated carry model totals 84.270B across message content before fixed prompts, compared with 60.757B recorded input. Use ledger-denominated scenario shares for prioritization.
- The temporary audit runner used a single read-only SQLite transaction, with supplemental queries for attribution. It removed content excerpts and file arguments, truncated transcript IDs, separated mandatory Close events from ordinary subscriptions, and added exact-body deduplication, a check for tool-carried sub-agent fan-in, and cache and calibration diagnostics.
- The branch script writes text excerpts and full transcript IDs to JSON and can put text excerpts in Markdown. Its snapshot copy also leaves a full database behind, and the JSON output path can alias the input database. It applies no watermark in its carry or calibration model. The temporary run used distinct private destinations and never copied the production database.
