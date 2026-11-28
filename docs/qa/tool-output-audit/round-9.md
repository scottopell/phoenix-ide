# Round 9: price read_file numbering with a real tokenizer, then decide whether to continue

## Review of round 8 (reviewer)

Accepted. I agree on no bash cap: about 30% of capped outputs at 16 KiB feed the next step, and most requests have no confirmed reader. A general
cap would trade tokens for wrong or extra turns. The design notes (a bounded, exact span reader) go on
file if this is ever revisited.

## Candidate

`read_file` renders each line as `format!("{line_num:>6}\t{line}")` (`crates/phoenix-tools/src/read_file.rs`).
Two options:
- **(a) Drop the padding:** `{line_num}\t{line}`. Exact line anchors are kept, so information risk is close to zero.
  Round 4 put it at 0.50% by bytes.
- **(b) Range numbering:** a header such as `lines 120–180` and no per-line numbers. Round 4 put it at 1.26% by bytes, but it loses
  per-line anchors.

Byte estimates overstate whitespace savings, because a run of spaces usually tokenizes to a single token.

## Questions

1. **Price (a) and (b) with a real BPE tokenizer.** Use `o200k_base` via `tiktoken`, or another tokenizer
   available offline. Report the per-result token delta on a sample of at least 5,000 `read_file` results, stratified by
   size, then scale it with the reconstructed carry. Report the share of recorded input. If no tokenizer is available
   offline, say so, and give the byte figure with a "whitespace collapses" bound: count each padding run as one
   token.
2. **Do agents use per-line numbers?** After a `read_file` result, how often does the next assistant message or
   tool call contain a number that appears as a line number in that result (e.g. `sed -n 120,140p`,
   `offset`, "line 132")? This is the risk for (b).
3. **Goal check.** By GOALS.md, if the best remaining candidate saves less than 0.5% of recorded input, or has
   unacceptable information risk, we have reached **G1** and stop. Say whether that is the case, and whether
   anything you've seen in rounds 1–8 that is *not* on the queue beats 0.5%.

## Output

Append `## Results`, write `round-9.json`, vote (build (a), build (b), or stop at G1), and pass the baton.

## Results

### Method

Tokenized the complete recorded result, the unpadded result and the inclusive range-header result with `o200k_base` from the checksum-verified `tiktoken` vocabulary. The vocabulary was cached before the audit and the production replay ran offline. Both transforms preserve source text, source indentation, blank lines and the existing remaining-lines footer. The range variant replaces each explicit line number and separator with one header containing the first and last number. Unrecognized, already-clipped and discontinuous results are excluded from the tokenizer projection.

A deterministic reservoir samples up to 1,500 results in each of six stored-byte strata. The projection multiplies each stratum's exact sampled token savings per carried numbered line by the census count of those lines, using the reconstructed full-result carry schedule. The separate count expansion is a sensitivity that weights each sampled result by its stratum's sampling fraction. The intervals measure sampling variation in the line-weighted estimate. They exclude model-tokenizer differences, request framing, caching and altered clearing schedules.

The reference analysis compares numbered results that carried to the same turn's next assistant message or first tool-call batch. It reports exact positive integer matches, then narrows to explicit prose line references, tool fields, path references, `sed` ranges and patch hunk headers. The strictest check is a subsequent `read_file` offset within the original range on the same normalized file path. A match can come from earlier context. A model could recover a line number by counting from the range header, and the audit cannot measure how reliably this would happen.

### Numbering cost

There were 112,967 eligible continuous results, with 374.6M numbered lines carried across the reconstructed requests. The sample has **7,663 results** across all six size strata. The 86 clipped or discontinuous results contributed another 2.7M carried numbered lines and receive no savings credit.

| Stored size | Eligible results | Tokenized | Unpadded tokens saved per result, median / p90 | Range header tokens saved per result, median / p90 |
|---|---:|---:|---:|---:|
| Under 1 KiB | 4,732 | 1,500 | 36 / 50 | 53 / 83 |
| 1–4 KiB | 40,533 | 1,500 | 100 / 160 | 176 / 296 |
| 4–16 KiB | 57,250 | 1,500 | 280 / 520 | 524 / 919 |
| 16–32 KiB | 8,654 | 1,500 | 800 / 1,080 | 1,370 / 2,011 |
| 32–64 KiB | 1,635 | 1,500 | 1,520 / 2,180 | 2,788 / 3,992 |
| 64 KiB or more | 163 | 163 | 2,800 / 4,000 | 5,396 / 7,465 |

| Option | Carried input saved | Share of 61.087B recorded input | Sampling interval | Count-expansion sensitivity |
|---|---:|---:|---:|---:|
| (a) Drop padding, keep every line number | **734.4M** | **1.20%** | 730.0–738.8M | 731.6M |
| (b) Inclusive range header, remove per-line numbers | 1,305.6M | 2.14% | 1,297.3–1,313.8M | 1,300.5M |

Dropping padding saves 1.96 tokens per carried numbered line on this tokenizer. A three-space prefix before a three-digit line number produces two space tokens in the tested encoding, including a separate final space before the digits. The earlier 0.50% byte-based estimate on rebased input was low. The unpadded variant grew zero sampled results. The extra header made 110 sampled results larger under the range variant. These estimates apply when results use the candidate rendering across the observed request history. An emission-only change accumulates the savings as new results replace padded history.

### Observed line references

Out of 112,822 numbered reads that carried at least once, 111,295 had a same-turn follow-up. A subsequent assistant message or tool batch matched a number in 33,506 reads (29.7%). An explicit line reference matched in **15,127 (13.4%)**, including 14,773 tool references and 362 prose references. The two counts can overlap. The next tool batch reread the same file at an explicit offset within the returned range in **4,489 (4.0%)**. Counts include the retained numbered portions of clipped results.

### Vote and goal check

**Build (a), drop padding while keeping exact per-line numbers.** It exceeds the 0.5% threshold with a wide margin and preserves the information used by observed follow-up calls. Both viewer text parsers accept an unpadded numeric prefix. Keep the tab separator, the typed line metadata and source indentation. The incremental 571.2M modeled for range-only numbering would remove explicit anchors that agents regularly reuse. Keep per-line numbers.

G1 has not been reached. The modeled total with (a) and the two merged changes is **10.99%** on this ledger. It is **10.53%** using only capacity-confirmed browser savings. G2 would be met after implementation, exact confirmation and merge by quorum. The tokenizer and carry checks passed against the synthetic renderer, clipped range and line-reference fixtures.

Earlier rounds identified larger gross leads outside this queue. Round 1 estimated 5.92% for rewriting historical patch arguments, 1.01% for shortening sub-agent fan-in, 0.86% for paging large file reads and 0.58% for a smaller initial transcript-evidence page. Each still needs evidence on lost context, extra reads or provider replay. The separate injected-context audit modeled 1.30% for sub-agent fan-in, overlapping the tool audit estimate, and 1.14% for Coordinator event batching in its first-request scenario. Those estimates are leads for future work and carry no credit toward this goal.

## Reviewer response

Accepted. I agree with **(a)**, and I agree G1 is not reached. Implemented in `573c40ea8`:
`writeln!(output, "{line_num}\t{line}")`. The tab, source indentation and footer are unchanged. That is the exact
transform your tokenizer replay priced. A new test, `lines_are_numbered_without_padding`, pins the format. Both UI
parsers already accept `^\s*(\d+)\t`, and padded fixtures stay valid for stored history. ADR-295 records it.

**Reviewer vote:** agree to merge once CI and review are clean. Please confirm your vote at this head.
I'm opening the PR now so CI runs in parallel. Merging waits for your vote.
