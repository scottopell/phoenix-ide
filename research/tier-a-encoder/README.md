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

```bash
uv run model_logreg.py --retrain     # rung 2: train logreg + score on eval set
cd runtime-spike && cargo run --release   # W3: candle forward-pass latency
```

## Findings so far

Numbers on the 61-command stratified seed set (SAFE=23, RISKY=20, BLOCKED=18).

| Rung | BLOCKED recall | RISKY recall | danger FNR | danger FPR |
|------|---------------|--------------|-----------|-----------|
| 0 majority | 0.00 | 0.00 | 1.00 | 0.00 |
| 1 bash_check rules | 0.56 | 0.00 | 0.74 | 0.00 |
| 2 logreg (word+char ngram TF-IDF) | 1.00 | 1.00 | **0.00** | 0.04 |

**Rung 2 beats rung 1 in-distribution, but the result is template-bound.** The
synthetic corpus (W2) shares templates with the eval set, so logreg's 0.00 FNR
reflects learned *template vocabulary*, not shell semantics. An out-of-template
probe of novel dangerous verbs — `terraform destroy -auto-approve`,
`kubectl delete ns prod`, `find / -delete`, `history -c` — is waved through as
`SAFE`. This is the quantified case for rung 3: a BERT-mini pretrained on real
shell corpora learns semantics that generalize past the templates. Do not ship
rung 2 as the encoder; it is the bar rung 3 must clear on a *real* held-out set.

### Decisions resolved (task 29001 decision-points)

- **#1 inference runtime / ANE — RESOLVED: pure-Rust candle on CPU.** The W3
  spike measures a random-weight BERT-mini forward pass (batch=1) at p99 **2.07ms
  (seq 32) / 2.69ms (seq 64)** on Apple Silicon CPU with the Accelerate BLAS
  backend, single-threaded. This clears a low-single-digit-ms budget, so the
  ONNX→CoreML/ANE path is unnecessary complexity — inference stays a single Rust
  dependency with no model-conversion toolchain.
- **#5 latency budget — RESOLVED: ~3ms p99 is the floor, well within budget.**
  Sub-ms (the original hypothesis) is *not* achievable — eager per-op overhead
  for a 4-layer stack floors at ~2ms — but 3ms p99 on a synchronous pre-dispatch
  gate is fine.
- Two runtime gotchas the spike surfaced, load-bearing for the inference impl:
  the **gemm backend** (BLAS-backed vs candle's pure-Rust gemm) dominates latency
  far more than ANE-vs-CPU would; and **batch=1 must pin rayon to one thread** —
  the multi-thread default is contention-bound and blows p99 to ~100ms.

### Still open (need W2-real before rung 3)

Decision-points #2 (tokenizer), #3 (real training corpus + per-class metrics on a
*real* held-out set), #4 (threshold calibration on a held-out split), #6 (model
shipping/versioning) remain. The synthetic corpus is a scaffold, not the training
set — rung 3 needs the real corpora named under W2.
