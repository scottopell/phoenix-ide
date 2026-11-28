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
