# Tool-output audit: goals and round-1 brief

## Why

The injected-context audit (`docs/qa/injected-context-audit-2026-11-27.md`) showed
that injected messages account for only about 2% of recorded input. The carry is dominated by
**tool results (≤86%)** and **assistant messages (11%)**, a large part of which is
tool-call *inputs* such as patch bodies. Phoenix implements every tool natively, so we can change any tool's output
format, caps or clearing behavior. Round 1 must tell us **which tool, which output
shape, and which change** would save the most input tokens without hiding information the agent needs.

The deliverable is a ranked list of concrete tool changes, each with an expected saving as
a share of recorded input (the ledger total, 60.76B in the last audit) and a confidence level.
Examples of what a row might look like: "bash: strip ANSI and collapse `\r` progress
lines, saving X%"; "read_file: drop the line-number prefix width, saving Y%";
"cargo test: keep failures only, saving Z%"; "lower the keyword_search cap from 128KB to N,
saving W%". If the honest answer is "no tool change is worth more than 1%", say so.

## Ground rules

Follow the privacy rules in `BATON.md`. Write the analysis as `scripts/tool_output_audit.py`
(uv, stdlib only, read-only; reuse helpers from `scripts/injected_context_audit.py` as
you see fit). Write results to `docs/qa/tool-output-audit/round-1.md` and `round-1.json`.

## Step 0: reconstruct clearing (do this first; everything else depends on it)

The database keeps only the final `clear_watermark`, so the last audit could give only
an upper bound. Recover the clearing events from the usage ledger: within each transcript,
a drop in recorded total input between consecutive requests, `input + cache_creation + cache_read`, is
a clearing or continuation event. Using the per-transcript token-per-byte fit, attribute
each drop to the clearable tool results it removed (those below the watermark at that point,
oldest first), and from that derive **when each result stopped being carried**.

Report how well this works: the share of drops explained, and the residual. Then give carry for every
later step as **reconstructed (best estimate)**, alongside the upper bound. Results from
non-clearable tools are carried exactly until the transcript ends; split them out.

## Questions to answer

1. **Per-tool inventory**: calls, error rate, and output size (bytes and estimated tokens:
   p50, p90, p99, max). Give the total and its share of all tool-result tokens, carry (reconstructed
   and upper bound), and whether the tool is clearable. Count images separately.
2. **Bash by command family.** Label each command by its first program and subcommand,
   using this fixed allowlist: `cargo {build,check,test,clippy,nextest,fmt,run,other}`,
   `git {diff,log,show,status,grep,other}`, `rg`, `grep`, `cat`, `sed`, `head`, `tail`,
   `ls`, `find`, `tree`, `npm`, `pnpm`, `npx`, `pytest`, `python`, `uv`,
   `./dev.py {check,up,restart,other}`, `curl`, `gh`, `jq`, `make`, `docker`, and `other`.
   Treat a pipeline as its first command and report a `has_pipe_filter` flag when the
   pipeline ends in `head`, `tail`, `grep`, `rg`, `wc` or `jq`. Per family, give the same
   size and carry stats as in Q1.
3. **Removable bytes by transform.** For every tool output, compute what each candidate
   transform would remove, then aggregate per tool and per bash family:
   - ANSI escape sequences;
   - `\r`-overwritten progress lines (keep only the last segment);
   - exact consecutive duplicate lines, collapsed to "line ×N";
   - runs of blank lines collapsed to one;
   - trailing whitespace;
   - `read_file` line-number prefixes, at the current width and at a minimal one;
   - JSON whitespace, for outputs that parse as JSON (minified size);
   - base64-like runs of 200 bytes or more;
   - single lines longer than 2,000 characters, truncated to 500;
   - cargo/rustc output: bytes in `Compiling …`/`Checking …`/`Downloaded …` lines and
     in warning blocks, and bytes in `test … ok` lines in passing test runs.

   Also report the gzip ratio as a rough measure of redundancy. Give removable bytes as a share of
   that tool's bytes and its reconstructed carry share of recorded input.
4. **Redundant calls.** Within a transcript:
   - the same tool and normalized input (hash) called again with an identical output hash;
   - `read_file` of the same path (hashed) over overlapping ranges with no intervening
     `patch` or write to that path;
   - repeated `git status`/`git diff` with an identical output.

   Give the carry wasted by each pattern.
5. **Hitting caps.** For each known cap, count outputs at or near it, and how often the
   *next* call in the same family narrows the query (a pipe filter, a smaller `lines`/`limit`, a more
   specific search). A high narrow-after rate means the first output was wasted. Caps: `read_file` 2,000 lines,
   `keyword_search` 128KB, tmux 128KB, bash tail default 200 lines and the 64KB ring,
   `patch` 16KB diff, and live progress 24KB. Check the current values in `crates/phoenix-tools/src`.
6. **Tool-call inputs inside assistant messages.** Split assistant carry into text,
   thinking or reasoning, and `tool_use` input per tool (patch bodies, bash commands,
   spawn prompts). These are never cleared. Which tool's inputs cost the most to carry, and how
   large is a typical patch input compared with the diff the tool reports back?
7. **Errors.** Error outputs per tool: size, and how often the identical failing call is retried.
8. **Anything else large** that the questions above don't cover. Name it, with numbers, and suggest how to measure it in round 2.

## Report format

Lead with the ranked table: change, tool or family, removable share of recorded input
(reconstructed and upper bound), confidence, and a risk note on what information the
agent would lose. Then answer Q0–Q8 in order, each with its numbers. Keep the report under 150 lines.
List the script's limits at the end.

## Reviewer side (in parallel, no database needed)

The reviewer measures the fixed per-request overhead from code: the size of each tool's
description and schema, and of the system prompt sections. The last audit put this at about 18.7k tokens per request,
roughly 17% of input. It will go in `docs/qa/tool-output-audit/overhead.md`.
