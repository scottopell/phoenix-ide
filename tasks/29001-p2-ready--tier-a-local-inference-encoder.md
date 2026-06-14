# Tier A — local on-device dangerous-action encoder

Implementation-ready spec for the **intent-agnostic** risk classifier that sits
behind the DenyGate seam. This task is the follow-up to the DenyGate
deterministic-deny refactor; it does NOT ship in that PR. Build it on top of the
`DenyGate` / `CheckedToolCall` seam that the refactor establishes.

## Why this exists

Two different problems gate a tool call:

- **Dangerous** — a property of the action *alone*. `rm -rf /`, a force-push, a
  payload piped to `sh`. Decidable from the command string + environment trust,
  with no reference to what the user asked for. A shell-tokenized encoder can do
  this.
- **Overeager** — a property of the action *relative to user intent*. The action
  is intrinsically fine but unauthorized: "clean up my branches" does not
  authorize a batch delete. Structurally needs the transcript and reasoning.
  This is Tier B and is explicitly OUT OF SCOPE here.

Tier A answers only "is this action intrinsically dangerous?" It is reasoning-blind
by construction — it never sees the transcript, so it cannot be prompt-injected
through tool results and needs no conversation context threaded into it. That
property is why it can live behind the existing intent-blind `ToolContext` /
DenyGate seam with zero new plumbing.

## Where it slots

The DenyGate refactor establishes this decision sequence at the executor
chokepoint (`dispatch_tool_execution`, synchronous, before the tool task spawn):

1. **Deterministic deny** (Layer 0, typed Rust rule registry) — hard guarantee,
   already shipped by the refactor. Force-push, blind `git add`, dangerous `rm`.
2. **Tier A encoder** (THIS TASK) — runs only if Layer 0 returned Allow. Soft
   classification of intrinsic danger. SAFE passes through; RISKY/BLOCKED produce
   a `Denial`.

Both stages are pure `(tool_name, input) -> verdict` functions. Tier A extends
`DenyGate::check` with a second stage; on a pass it still returns the same
`CheckedToolCall` proof token, on a block the same `Denial` type. No new seam, no
signature change beyond what the refactor already introduced. The escalation
counter (consecutive/total denials) the refactor adds on the executor covers
Tier A denials identically — a Tier A block is deny-and-continue just like a
Layer 0 block.

## Model

Closest published analog: Notaro et al., "Command-line Risk Classification using
Transformer-based Neural Architectures" (arXiv:2412.01655) — BERT-mini (4 hidden
layers, 4 attention heads, hidden size 256, ~20k vocab), BPE tokenizer trained
from scratch on a Bash corpus, self-supervised pretrain (masked-LM +
next-command-prediction) then a fine-tuned classification head. Designed
explicitly to replace a rule-based gate at inference time. This is nearly the
exact spec; treat it as the reference architecture, not a mandate.

**Risk tiers** (Notaro's three, adopt as-is):
- `SAFE` — read-only / no significant state change. Pass.
- `RISKY` — may irreversibly alter state; needs escalation. Soft deny.
- `BLOCKED` — will irreversibly alter state; must never execute. Deny.

**Input**: the action string. v1 = the bash command (the `cmd` already handed to
`bash_check`). Generalize later to other tool calls by serializing
`(tool_name, input)` to a canonical string. Optionally a trust bit for the
environment (current worktree vs outside), matching Notaro's optional env input.

**Output**: one of the three tiers + a confidence. DenyGate maps RISKY/BLOCKED to
`Denial`, SAFE to a cleared `CheckedToolCall`.

**Reasoning-blind**: input is the action only. No transcript, no user messages, no
tool results. This is a hard architectural constraint, not a default — it is the
injection-defense property and the reason Tier A needs no context plumbing.

## Decision points the implementing PR MUST resolve

These are unresolved and need an explicit owner-decision (do not default silently):

1. **Inference runtime on Apple Silicon / ANE.** Candidates: `candle` (pure-Rust,
   Metal backend, no ANE), `ort`/ONNX Runtime (CoreML execution provider → ANE),
   or a CoreML `.mlpackage` via `objc2`/a thin FFI shim. ANE access realistically
   means CoreML EP. Decide: do we require ANE, or is Metal/CPU latency acceptable?
   A BERT-mini forward pass is sub-ms on CPU for a single short command — ANE may
   be unnecessary complexity. Measure before committing to CoreML.
2. **Tokenizer.** Train a Bash BPE from scratch (Notaro's finding: general-text
   and char-level tokenizers adapt poorly to shell syntax) vs. reuse an existing
   tokenizer. From-scratch is the published-better path but adds a training
   artifact to ship and version.
3. **Training corpus.** Candidates: Notaro's 71,164-script pretrain corpus
   (~500MB) if obtainable; the open labeled set of 13,446 shell commands from 175
   cybersecurity-training participants (Bash/ZSH/Metasploit, with timestamp +
   working-dir + host metadata, ScienceDirect S2352340921006806); and
   Phoenix-internal: synthesize/label from our own tool-call logs. Class imbalance
   is the central difficulty — dangerous commands are rare, so a trivial
   high-accuracy classifier exists; **evaluate precision/recall on the BLOCKED/RISKY
   classes specifically**, not aggregate accuracy.
4. **Threshold calibration + targets.** Set the SAFE→RISKY and RISKY→BLOCKED cut
   points and state explicit FNR/FPR targets per class. Because this is a soft
   layer behind a hard deterministic-deny floor and in front of deny-and-continue,
   it can run tuned for low FNR (err toward blocking) — a false positive costs one
   retry + a nudge, not a dead session. Quantify the acceptable FPR given the
   deny-and-continue escalation thresholds.
5. **Latency budget.** Tier A runs synchronously in `dispatch_tool_execution`
   before the spawn — it is on the critical path of every consequential tool call.
   State a hard budget (target: low single-digit ms p99) and a fallback: if
   inference exceeds budget or the model fails to load, fail OPEN to Layer 0 only
   (deterministic deny still holds) and log at debug — never block the tool on an
   encoder timeout, never silently drop the encoder without a log line.
6. **Model shipping + versioning.** Embedded in the binary (RustEmbed, like the
   UI) vs. downloaded/cached. Size budget. How a model update is versioned and
   rolled out.

## Correct-by-construction constraints

- Tier A output feeds the SAME `CheckedToolCall` mint / `Denial` types as Layer 0.
  An ungated call stays unrepresentable; the encoder is a second filter before the
  proof token is minted, not a parallel path that can be skipped.
- The encoder MUST NOT take the transcript as input. Enforce structurally — its
  input type is the action string (+ optional env trust bit), with no field that
  could carry conversation context. This makes "reasoning-blind" a type fact, not
  a discipline.
- Encoder-unavailable (load failure / timeout) is a logged capability gap at
  debug+, never a silent pass and never a hard block. Fail open to Layer 0.

## Out of scope (do NOT build here)

- **Tier B** — overeagerness / consent-scoping. Needs the transcript and a
  reasoning-blind local instruction-following LLM. Separate task; this is the
  layer where the "executor pre-dispatch hook vs. transcript-slice-into-ToolContext"
  decision becomes load-bearing. Tier A deliberately avoids touching it.
- Declarative/config-scoped rule representation. Layer 0 stays a typed Rust
  registry per the DenyGate refactor decision.
- Any change to the `ToolContext` shape. Tier A needs nothing new on it.

## Acceptance

- A `DenyGate` second stage that, given a bash command Layer 0 allowed, returns a
  tier and converts RISKY/BLOCKED to a `Denial` carrying a model-grounded reason.
- Reasoning-blind by type: no transcript reachable from the encoder's input.
- Fail-open-to-Layer-0 on encoder unavailability, with a debug+ log line.
- Per-class (BLOCKED/RISKY) precision/recall reported against a held-out set, with
  the chosen FNR/FPR targets stated and met.
- p99 inference latency within the stated budget, measured on-device.
