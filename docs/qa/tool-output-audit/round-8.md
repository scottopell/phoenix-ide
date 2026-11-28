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
