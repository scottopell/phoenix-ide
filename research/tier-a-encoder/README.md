# Tier A encoder — model development workspace

Research/training workspace for the **intent-agnostic on-device dangerous-action
classifier** (task `29001`). The shipped artifact is a small shell-command risk
classifier that slots behind the `DenyGate` seam (task `29002`) as a second
stage: Layer 0 deterministic deny first, then this soft classifier.

This directory is the **training/research half**. It produces a model artifact +
calibrated thresholds. The **inference half** (loading the model in Rust behind
`DenyGate::check`) lives in the crate tree and is out of scope here until a model
clears the baseline ladder below.

## The governing discipline: eval-first, baseline ladder

The central difficulty is **class imbalance** — dangerous commands are rare, so a
classifier that always answers `SAFE` scores ~98% accuracy and is worthless.
Therefore:

> Never train before you can measure. Never measure with aggregate accuracy.

Everything climbs one frozen eval harness. Per-class precision/recall on the
`BLOCKED`/`RISKY` classes is the only score that counts; aggregate accuracy is
banned from every report.

### The ratchet — each rung must beat the one below on the *same* eval set

| Rung | What | Why it's here |
|------|------|---------------|
| 0 | majority-class (always `SAFE`) | exposes the imbalance lie; the floor |
| 1 | `bash_check` rules-as-classifier | mirrors the deterministic Layer 0; doubles as the real fallback, so its numbers matter twice |
| 2 | token n-gram + logistic regression | the "a neural model must beat this" bar |
| 3 | BERT-mini (spec target) | only worth the training cost if it clears rung 2 |

`eval.py` already runs rungs 0 and 1. A new model is added as a callable and
scored against the identical frozen set — no model is "better" until the harness
says so.

### Threshold calibration

Cut points (`SAFE→RISKY`, `RISKY→BLOCKED`) and FNR/FPR targets are tuned on a
**separate calibration split**, never the test set. Final numbers are reported on
the frozen test set exactly once per candidate. Because Tier A sits behind a hard
deterministic floor and in front of deny-and-continue, it tunes for **low FNR**
(err toward blocking) — a false positive costs one retry + a nudge, not a dead
session.

## Workstreams

W1 and W3 are independent and parallelizable now; W2 feeds W4→W5.

- **W1 — eval spine** *(this scaffold)*: frozen labeled set + metrics harness +
  baseline numbers. Correct under every downstream choice; needs neither the seam
  nor a trained model.
- **W2 — data pipeline**: corpus acquisition (Notaro 71k pretrain corpus; the
  open 13,446-command labeled set, ScienceDirect S2352340921006806;
  Phoenix-internal tool-call logs), label schema, **rare-class augmentation**.
- **W3 — runtime spike**: candle BERT-mini forward pass with *random weights*,
  measure on-device p99. Resolves decision-points #1 (runtime/ANE) + #5 (latency)
  **before** investing in training. If sub-ms on CPU with candle, the
  ONNX-CoreML/ANE path is unnecessary complexity — measure first.
- **W4 — tokenizer**: Bash BPE trained from scratch (Notaro: general-text and
  char-level tokenizers adapt poorly to shell syntax).
- **W5 — model**: self-supervised pretrain (masked-LM + next-command-prediction)
  then a fine-tuned classification head.

## Label schema

Three tiers, adopted from Notaro et al. as-is. See `labels.md`.

- `SAFE` — read-only / no significant state change. Pass.
- `RISKY` — *may* irreversibly alter state; needs escalation. Soft deny.
- `BLOCKED` — *will* irreversibly alter state; must never execute. Deny.

## The eval set caveat

The shipped **test** set must reflect the real-world prior (overwhelmingly
`SAFE`) so reported FPR is honest. The **dev seed** set (`data/eval_seed.jsonl`)
is deliberately *stratified* — over-sampled on `RISKY`/`BLOCKED` — for signal
during early iteration. Do not read its class balance as the production prior.

## Running

```bash
uv run eval.py                      # score all built-in baselines on the seed set
uv run eval.py --data data/eval_seed.jsonl
```

No dependencies — pure stdlib. Adding a model means adding a classifier callable
and re-running; the harness and frozen set do not change.
