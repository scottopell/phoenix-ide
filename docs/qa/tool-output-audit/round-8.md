# Round 8: capping very large bash results (risk first)

## Status

PR #1206 is merged (`ad855fe8d`). Running total is about 9.8% against the 10% goal (G2).

## Candidate

Your round 4 ranked "bound bash output at 16 KiB with retrieval" first on the new baseline (a 6.4% gross ceiling).
The design under consideration:

- The model sees at most **N KiB** of a bash `run` result: the first H bytes and the last (N−H) bytes,
  with one marker line stating how much was omitted and how to retrieve it.
- **The full output stays in the stored transcript.** The cap applies only to what is sent to the model, as in
  ADR-290's split between the model view and the stored display. Retrieval goes through `read_conversation
  @self#seq-<n>` from #1204, so it does not depend on the in-memory ring, which can expire.

The money is not in doubt; whether the agent loses what it needs is. Measure the risk before I
build anything.

## Questions (report aggregates only; no text)

1. **Where does the signal sit?** For bash results over 16 KiB, find lines matching common diagnostics
   (`error`, `Error`, `FAILED`, `panicked`, `Traceback`, `warning:`, `failed`, `✗`, `assert`).
   Report the distribution of their *relative position* in the output (deciles), split by bash
   family. Report the share of such outputs whose diagnostic lines fall entirely within the first 4 KiB, or entirely within
   the last 12 KiB.
2. **What the agent used next.** For results over 16 KiB, take the next assistant message and tool call.
   Extract tokens of at least 6 characters (paths, identifiers, numbers with at least 4 digits). Report how often one of them occurs in the
   result **only** within the bytes a given cap would omit. Try (N, H) = (8, 2), (16, 4), (32, 8) KiB. This
   is the "would have lost evidence" rate.
3. **Savings at each cap,** using the reconstructed clearing schedule: token-turns saved, net of the
   marker, plus an estimated retrieval cost (one `read_conversation` per lost-evidence case, priced at the
   omitted bytes).
4. **Families to exempt:** if one family (e.g. `git diff`, `cat`, `sed -n`) has a high
   lost-evidence rate, it is a candidate for exemption. Rank families by saving divided by lost-evidence rate.

## Output

Append `## Results`, write `round-8.json`, recommend a cap (or no cap), vote, and pass the baton.

## Results

### Method

The replay reads decoded stdout and stderr line text. Historical structured responses are converted to the compact `lines` shape and the run command echo is removed before pricing. Untyped or already-truncated stored text contributes to reconnaissance only. Savings use structured `run` results with at least one reconstructed full-result carry. The budgets are output bytes before JSON framing. The candidate keeps UTF-8 boundaries and prices the omission marker, separate head and tail ranges, and tail line offset. Current non-output fields are retained.

Diagnostic deciles use each matching line's starting byte as a fraction of result length. Head and tail coverage require the full matching line. A result counts as potentially losing evidence when the next assistant message or the first subsequent tool-call batch has an exact lexical match wholly inside the removed window and no whole match in either kept range. This checks the current turn through the next user or continuation boundary. The command-excluded count removes tokens already present in the originating bash input. Generic lexical matches can overstate use, prior context and parallel results can carry the same detail, and concepts paraphrased or needed later can be missed. Diagnostic keywords can also occur in source or data rather than a process diagnostic.

Savings use the reconstructed clearing schedule and estimated token-per-byte slope. The first retrieval estimate subtracts one omitted slice per case with potential evidence loss. The additional-request sensitivity also subtracts the next measured input after the cap. Later carry of the recovered output, altered clearing decisions, and the actual behavior of an agent encountering a cap remain unmeasured. Reader availability is inferred from positioned tool policy or the latest advertised set.

### Where the signal sits

The snapshot contains 31,109 bash results over 16 KiB, including 29,812 `run` results. Of those 31,109, 29,813 had at least one diagnostic-keyword match. The table shows the largest families and selected build families. The JSON includes every family. The last three columns use only results with at least one diagnostic match.

| Family | Results over 16 KiB | Matching lines | Share of matching lines from first to tenth byte decile | All in first 4 KiB | All in last 12 KiB | All in either kept range |
|---|---:|---:|---|---:|---:|---:|
| All | 31,109 | 607,314 | 8.9, 9.1, 9.3, 9.6, 9.8, 10.0, 10.2, 10.5, 10.6, 12.0% | 1.2% | 9.1% | 20.4% |
| Other | 13,176 | 229,931 | 9.3, 9.1, 9.4, 9.5, 9.7, 10.0, 10.2, 10.5, 10.5, 11.7% | 1.3% | 8.3% | 18.1% |
| `sed` | 4,585 | 106,824 | 9.2, 9.8, 9.9, 10.1, 10.1, 10.1, 9.9, 10.0, 10.2, 10.5% | 1.0% | 7.9% | 20.4% |
| `git diff` | 3,136 | 83,027 | 7.7, 8.8, 9.3, 9.3, 9.8, 10.3, 10.5, 11.0, 10.9, 12.3% | 0.8% | 7.3% | 16.7% |
| `rg` | 3,092 | 54,494 | 8.6, 8.8, 8.9, 9.2, 9.7, 10.3, 10.7, 10.9, 10.8, 12.0% | 1.3% | 10.6% | 24.1% |
| `cat` | 1,419 | 33,177 | 9.0, 9.5, 9.5, 9.1, 9.5, 9.8, 9.8, 10.4, 11.1, 12.2% | 0.9% | 9.0% | 19.3% |
| `git show` | 1,413 | 29,806 | 8.4, 9.0, 8.7, 10.2, 10.0, 9.5, 10.2, 10.3, 11.3, 12.4% | 1.4% | 10.2% | 22.1% |
| `cargo fmt` | 299 | 5,855 | 9.6, 8.5, 7.6, 7.3, 8.4, 8.5, 8.7, 9.2, 10.3, 21.8% | 2.4% | 24.5% | 52.0% |
| `cargo test` | 139 | 3,701 | 7.6, 7.9, 8.8, 14.0, 10.2, 9.7, 9.9, 9.2, 7.9, 14.8% | 1.5% | 34.6% | 47.4% |

Matches occur throughout the output. The first 4 KiB and last 12 KiB together preserve all matching lines in only one fifth of outputs with matches. The profile remains broad even in the higher-volume families.

### What the next agent step used

The denominator for the percentages in the third column is all 31,109 results over 16 KiB. There were 30,512 results with a same-turn follow-up. At 32 KiB, 9,747 results exceed the cap, so the 2,776 potential losses also equal 28.5% of results that would actually be capped.

| Cap and head | Results over 16 KiB that would be capped | Potential losses, share of all over 16 KiB | After excluding originating command terms | Hits in next tool arguments |
|---|---:|---:|---:|---:|
| 8 / 2 KiB | 31,109 | 16,018 (51.5%) | 15,053 | 15,064 |
| 16 / 4 KiB | 31,109 | 9,663 (31.1%) | 9,057 | 8,887 |
| 32 / 8 KiB | 9,747 | 2,776 (8.9%) | 2,652 | 2,495 |

On structured `run` results carrying to the model, 8,499 of the 28,008 results at 16 KiB had a potential evidence loss (30.3%). **7,020 of those 8,499** lacked a confirmed reader. In the compact-format subset, 180 of 751 results still had a potential loss (24.0%). At 32 KiB the loss proxy remained present in 1,857 of 7,621 results that would be capped (24.4%).

### Savings with retrieval

| Cap and head | Structured `run` results priced | Savings after marker | One omitted-slice retrieval per hit | Net input saved (% of recorded) | Net with extra model request | Net on requests with known reader |
|---|---:|---:|---:|---:|---:|---:|
| 8 / 2 KiB | 63,131 | 2,821.5M | 96.8M | 2,724.7M (4.46%) | 841.3M | 632.8M |
| 16 / 4 KiB | 28,008 | 1,405.9M | 42.5M | 1,363.3M (2.23%) | 641.5M | 293.0M |
| 32 / 8 KiB | 7,621 | 402.8M | 9.8M | 393.0M (0.64%) | 216.5M | 89.2M |

Recorded input is 61.075B. After the previously modeled clearing and browser savings, the 16 KiB estimate is 2.47% of rebased input, or 1.16% when charging the extra model request. With a confirmed reader, it falls to 293.0M before the extra model request, 0.48% of recorded input. The estimated 16 KiB gross is lower than the Round 4 ceiling. This replay prices the compact stdout shape, excludes non-run results and 1,799 already-truncated stored results, preserves other result fields, and charges the marker.

The exact-message reader resolves the stored output and returns up to 32 neighboring messages on either side. Its response can hit the existing 100 KiB tool-result truncation and there is no target stdout slice selector. The one-slice price above assumes a retrieval operation with tighter bounds than this interface currently provides. Current transcript retention also inherits the ring window and existing result truncation.

### Family ranking and vote

Ranked by net million input tokens saved per percentage point of potential evidence loss at 16 KiB. The compact JSON carries the full family counts. The `other` bucket combines unrelated commands and cannot serve as an eligibility rule.

| Rank | Family | Capped results | Potential evidence loss | Net input saved | Millions saved per percentage point of loss |
|---:|---|---:|---:|---:|---:|
| 1 | Other | 11,438 | 35.0% | 623.7M | 17.8 |
| 2 | `sed` | 4,509 | 27.9% | 165.6M | 5.9 |
| 3 | `rg` | 2,950 | 23.4% | 108.4M | 4.6 |
| 4 | `git diff` | 2,931 | 27.7% | 126.9M | 4.6 |
| 5 | `git status` | 984 | 25.9% | 69.0M | 2.7 |
| 6 | `cat` | 1,375 | 30.6% | 65.3M | 2.1 |
| 7 | `gh` | 718 | 33.3% | 69.6M | 2.1 |
| 8 | `git show` | 1,361 | 27.6% | 42.8M | 1.6 |
| 9 | `cargo test` | 30 | 3.3% | 2.9M | 0.9 |
| 10 | `find` | 237 | 20.3% | 16.8M | 0.8 |

**Vote against a general bash result cap in this round.** Exempt reads, searches and diffs, including `cat`, `sed`, `rg`, `git diff` and `git show`. Their outputs are frequently the source of the agent's next arguments. A narrower cohort of simple successful `cargo build`, `cargo check`, `cargo clippy`, and `cargo fmt` commands had only three structured `run` results eligible even at 8 KiB, all without reader evidence. It offers no measured case for a production cap. Pursue compact `read_file` numbering next, with a replay that preserves exact absolute line numbers and matches how models use them.

If the bash cap is revisited, qualify an exact, bounded read of the omitted span on the same callable surface. Preserve metadata and head/tail offsets, and use the same projection for request rendering and capacity accounting. Validate that the recoverable output survives restarts and gets to the model without the general reader's neighboring content. Replay fixtures passed for UTF-8 boundaries, exact token placement, diagnostic offsets, legacy decoding, reader availability, and cap accounting.
