# Round 1 tool-output economics

| Candidate | Tool or family | Reconstructed / upper | Confidence | Information risk |
|---|---|---:|---|---|
| Compact historical patch arguments after execution | patch | 5.92% / 5.92% | Low | Illustrative 75% removal. Provider replay and exact change recovery need validation. |
| Retain a bounded result with full-output retrieval | bash | 5.86% / 31.10% | Low | Large windows can contain decisive diagnostics, and discarded ring history can expire. |
| Shorten very long output lines with retrieval | bash | 2.01% / 14.55% | Low | Can cut structured records, diff lines, or diagnostics. |
| Use range headers with numbering at anchors | read_file | 1.15% / 3.65% | Low | Gross ceiling before new headers. Exact line references need a stable presentation. |
| Use a typed reference for echoed commands | bash | 1.13% / 6.63% | Medium | Handle reads need enough identity to distinguish concurrent commands. |
| Use a shorter fan-in with full child evidence available | spawn_agents | 1.01% / 1.01% | Medium | A shorter report can omit blockers and obligations. |
| Page large file reads at a smaller byte budget | read_file | 0.86% / 2.01% | Low | May increase tool calls and hide relevant lines on the first read. |
| Reduce the initial evidence page | read_conversation | 0.58% / 5.16% | Low | May require an extra read to verify history. |
| Remove line-number padding | read_file | 0.46% / 1.42% | Medium | Line anchors stay visible, but tokenizers may already encode runs of spaces cheaply. |
| Page large structured results | query_database | 0.41% / 5.62% | Low | Requires stable paging and whole-result access under the same authority. |

All savings percentages use **60.790B recorded input tokens** across 566,530 ledger rows. Candidates overlap. Carry estimates price removed bytes before extra retrieval, changes to clearing frequency, or cache effects.

## Q0 · Clearing reconstruction

- 561,636 positive-usage response anchors account for 60.237B input in 12,698 transcripts. The ledger also includes requests without a response anchor.
- The model explains 13,770 of 16,455 net drops (83.68%) and 97.12% of drop volume. Absolute residual across all drops is 1.6k tokens at p50 and 6.5k at p90 after accounting for additions.
- 7,269 anchored sweeps and 6,737 inferred sweeps assign a clearing time to 233,100 of 238,820 clearable rows under final watermarks. 519.3M token-turns for eligible rows remain at full carry because timing was unresolved.
- Matched 182,238 of 184,821 ordinary capacity-linked ledger rows to response usage. Of 7,315 matched reduction requests, 7,269 fitted the recorded footprint. Matched reduction residual p50/p90 is 48/159 tokens.
- Tool carry is **29.737B reconstructed** versus **108.008B** without clearing. Clearable and nonclearable results account for 20.298B and 9.439B. Replacement markers add a separate 2.127B.

## Q1 · Tool inventory

All tools produced 590,795 result rows and 1,285 image blocks. Per-tool detail and individual quantiles are in the accompanying JSON.

| Tool | Results | Errors | Output-token share | Bytes p50/p90/p99/max | Tokens p50/p90/p99/max | Clearability | Carry and upper |
|---|---:|---:|---:|---:|---:|---|---:|
| bash | 234,536 | 0.31% | 61.21% | 5.0k/28.5k/89.2k/194.0k | 1.4k/7.5k/23.7k/54.7k | clearable | 11.714B / 60.722B |
| read_file | 114,222 | 0.91% | 19.59% | 5.1k/15.7k/38.8k/102.4k | 1.3k/4.1k/10.1k/33.9k | clearable | 4.994B / 16.120B |
| patch | 71,597 | 10.90% | 3.29% | 1.4k/4.2k/7.6k/17.2k | 380/1.1k/2.1k/5.0k | nonclearable | 4.734B / 4.734B |
| spawn_agents | 7,815 | 3.16% | 1.36% | 4.0k/15.9k/53.3k/155.6k | 1.1k/4.3k/14.2k/43.3k | nonclearable | 2.657B / 2.657B |
| skill | 3,930 | 1.37% | 0.79% | 8.5k/12.6k/29.5k/30.0k | 2.3k/3.5k/8.4k/9.8k | mixed | 1.579B / 1.581B |
| search | 72,225 | 1.88% | 6.46% | 921/12.0k/33.9k/102.4k | 241/3.1k/8.6k/35.6k | clearable | 1.499B / 3.557B |
| query_database | 22,073 | 6.37% | 2.85% | 1.4k/11.1k/63.7k/102.4k | 428/3.2k/18.6k/38.1k | clearable | 1.126B / 9.808B |
| read_conversation | 5,522 | 4.49% | 3.40% | 7.3k/102.4k/102.4k/102.4k | 2.1k/28.7k/33.2k/38.3k | clearable | 683.5M / 6.831B |

## Q2 · Bash families

| Family | Results | Errors | Pipe filters | Bytes p50/p90/p99/max | Carry and upper |
|---|---:|---:|---:|---:|---:|
| other | 108,396 | 0.36% | 2,068 | 3.6k/27.2k/102.4k/194.0k | 5.025B / 31.925B |
| sed | 22,055 | 0.39% | 293 | 13.0k/35.5k/74.0k/102.4k | 1.514B / 5.267B |
| rg | 21,768 | 0.26% | 11,564 | 8.8k/28.0k/78.6k/102.4k | 1.106B / 4.210B |
| git diff | 10,683 | 0.23% | 929 | 15.1k/47.9k/102.4k/102.4k | 799.6M / 2.443B |
| gh | 13,205 | 0.11% | 1,116 | 2.3k/16.4k/59.4k/115.8k | 584.3M / 4.221B |
| git status | 8,612 | 0.34% | 117 | 3.1k/26.2k/62.9k/102.4k | 505.5M / 2.162B |
| cat | 5,764 | 0.29% | 618 | 11.7k/40.2k/86.9k/102.4k | 426.2M / 1.486B |
| git show | 5,942 | 0.27% | 1,078 | 13.3k/39.4k/98.4k/102.4k | 354.5M / 1.568B |

See JSON for every family, including token quantiles, output shares, images and gzip ratios. Family routing follows the first simple command and propagates to observed handle reads. Other accounts for 46% of Bash results and includes wrappers, unlisted programs, and unresolved handles.

## Q3 · Removable output

Whole-corpus gzip size is 30.09% of body bytes. Compression ratio measures repeated bytes and does not imply an LLM token saving.

| Transform | Results | Removed bytes | Share of all tool bytes | Reconstructed / upper |
|---|---:|---:|---:|---:|
| Very long lines | 19,910 | 388.3M | 9.10% | 3.49% / 27.22% |
| Whole line number prefix | 112,994 | 117.6M | 2.75% | 1.15% / 3.65% |
| Line number padding | 112,992 | 46.7M | 1.09% | 0.46% / 1.42% |
| JSON minification | 43,596 | 11.5M | 0.27% | 0.25% / 1.57% |
| Cargo progress | 6,789 | 7.0M | 0.16% | 0.10% / 0.67% |
| ANSI escapes | 5,774 | 5.7M | 0.13% | 0.07% / 0.42% |
| Trailing whitespace | 190,107 | 2.1M | 0.05% | 0.02% / 0.08% |
| Passing test lines | 791 | 2.5M | 0.06% | 0.02% / 0.20% |
| Consecutive duplicate lines | 12,507 | 1.9M | 0.04% | 0.02% / 0.10% |
| Long base64-like runs | 736 | 1.9M | 0.05% | 0.02% / 0.15% |
| Cargo warnings | 1,364 | 599.4k | 0.01% | 0.01% / 0.05% |
| Overwritten progress | 914 | 292.2k | 0.01% | 0.00% / 0.03% |
| Extra blank lines | 13,650 | 52.8k | 0.00% | 0.00% / 0.00% |

Per-tool and per-family byte and carry shares are in JSON. Transforms are independent and must not be added.

## Q4 · Redundant calls

- Identical normalized call and output · 2,891 later rows and 35.2M / 164.8M token-turns. Gross shares 0.06% / 0.27%.
- Overlapping unchanged read ranges · 8,583 later rows and 44.8M / 125.6M token-turns. Gross shares 0.07% / 0.21%.
- Repeated identical Git status or diff window · 45 later rows and 149.5k / 856.2k token-turns. Gross shares 0.00% / 0.00%.

## Q5 · Caps and next-call narrowing

| Cap signal | Near or hit | With a next result | Narrowed |
|---|---:|---:|---:|
| Read file 2,000-line default window | 14 | 14 | 9 (64.29%) |
| Read file explicit window | 93,645 | 87,616 | 17,985 (20.53%) |
| Keyword search 128 KiB visible proxy | 0 | 0 | 0 (—) |
| Keyword search scope-truncation marker | 40 | 0 | 0 (—) |
| Keyword search 8,000-byte raw fallback marker | 5 | 1 | 0 (0.00%) |
| Tmux 128 KiB visible proxy | 1 | 0 | 0 (—) |
| Bash 200-line tail | 479 | 414 | 21 (5.07%) |
| Bash explicit tail | 313 | 296 | 145 (48.99%) |
| Bash 64 KiB partial-line proxy | 1,837 | 1,473 | 89 (6.04%) |
| Patch 16 KiB diff | 28 | 27 | 0 (0.00%) |
| Patch diff truncation marker | 9,674 | 9,062 | 0 (0.00%) |
| Runtime 100 KiB body ceiling | 3,050 | 2,247 | 122 (5.43%) |

Near means at least 95% of the visible cap or a known truncation marker. The current Bash ring is 4 MiB. Its 64 KiB limit governs partial lines. Live 24 KiB progress ships over the UI stream, so persisted tool results cannot count its cap hits. Raw keyword and tmux caps sit upstream of the runtime body cap.

## Q6 · Assistant history

- Text · 1.981B token-turns (3.26%).
- Reasoning · 0 token-turns (0.00%).
- Other · 0 token-turns (0.00%).
- Serialization · 1.885B token-turns (3.10%).
- Largest tool-use inputs · patch 4.796B (7.89%), bash 4.581B (7.54%), send_conversation_message 1.325B (2.18%), spawn_agents 470.0M (0.77%), query_database 448.9M (0.74%).
- Among 62,047 paired patches with a diff, median input is 336 tokens and median emitted diff is 430. Median paired ratio is 0.82×.

## Q7 · Errors

- patch · 7,807 flagged result errors, 3.5M output bytes, and 131.1M carried tokens. 260 retries used an earlier failing input and 242 also failed.
- spawn_agents · 247 flagged result errors, 31.3k output bytes, and 2.6M carried tokens. One retry used an earlier failing input and 1 also failed.
- search · 1,360 flagged result errors, 256.4k output bytes, and 1.6M carried tokens. One retry used an earlier failing input and 0 also failed.
- skill · 54 flagged result errors, 38.8k output bytes, and 1.6M carried tokens. 3 retries used an earlier failing input and 3 also failed.
- read_file · 1,034 flagged result errors, 206.3k output bytes, and 1.2M carried tokens. 2 retries used an earlier failing input and 1 also failed.

Bash command exit failures are distinct from tool-level errors. The passing-line transform checks exit state when available.

## Q8 · Other leads and follow-up

- browser · 344 transcripts (2.71%) and 3,955 call-bearing measured requests (0.70%).
- present_svg · 4 transcripts (0.03%) and 10 call-bearing measured requests (0.00%).
- spawn_agents · 1,137 transcripts (8.95%) and 7,822 call-bearing measured requests (1.39%).
- tmux · 234 transcripts (1.84%) and 1,810 call-bearing measured requests (0.32%).
- Measure deferred browser definitions on eligible routes using first discovery, loaded-tool retention, and first-request latency. The 344 calling transcripts account for 52,615 measured requests, including 15,528 through each first call. Repository overhead needs a separate route- and role-matched comparison.
- Consider Bash command echo first for exact output replay. Check that each omitted echo can be recovered from retained call arguments or a stable handle reference.
- Compare settled Bash handle reads by overlapping ring offsets. The full-output hash misses repeated windows when metadata changes.

## Limits

- Median fitted text slope is 0.257 tokens per byte across 11,594 fitted transcripts. Text type, images, short outputs, and provider serialization can materially change the estimates. Images use 1,500 tokens each.
- Request drops also reflect route, guidance, replay, or bounded-history changes. Historical clearability uses current native tool rules. Prefix mismatches and requests outside the ledger join leave clearing time uncertain.
- Cap narrowing and repeat metrics use visible tool-result order. Search narrowing uses a strict substring or dropped-term proxy. Bash families approximate shell parsing and overlap invalidation covers only known or suspected in-transcript writes.
- Read aliases, external edits, ANSI semantics, JSON escaping differences, provider replay, and overwritten stored results can move the modeled savings. Gross upper bounds assume full retention of every stored output.
