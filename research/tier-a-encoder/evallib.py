# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Tier A eval library — the frozen metric, extracted for reuse.

Pure stdlib. Holds the per-class precision/recall + danger FNR/FPR scoring and
its reporting format, so every rung of the baseline ladder scores through the
SAME code. `eval.py` (rungs 0/1) and any model candidate both import from here;
the metric is defined once and does not fork per classifier.

Aggregate accuracy is deliberately absent: under the real-world class prior
(overwhelmingly SAFE) it is a vanity metric that hides the only failures that
matter — a dangerous command waved through (false negative on BLOCKED).
"""

from __future__ import annotations

import json
from collections import defaultdict
from pathlib import Path

TIERS = ["SAFE", "RISKY", "BLOCKED"]
SEVERITY = {"SAFE": 0, "RISKY": 1, "BLOCKED": 2}


def load_jsonl(path: str | Path, base: str | Path | None = None) -> list[dict]:
    """Read a {cmd,label} JSONL file. Relative paths resolve against `base`."""
    p = Path(path)
    if not p.is_absolute() and base is not None:
        p = Path(base) / p
    return [json.loads(line) for line in p.read_text().splitlines() if line.strip()]


# ---------------------------------------------------------------------------
# Metrics — per-class P/R + the danger-vs-safe FNR/FPR that drive calibration.
# ---------------------------------------------------------------------------
def score(classify, rows: list[dict]) -> dict:
    cm = defaultdict(lambda: defaultdict(int))  # cm[true][pred]
    for r in rows:
        cm[r["label"]][classify(r["cmd"])] += 1

    per_class = {}
    for t in TIERS:
        tp = cm[t][t]
        fp = sum(cm[o][t] for o in TIERS if o != t)
        fn = sum(cm[t][o] for o in TIERS if o != t)
        prec = tp / (tp + fp) if tp + fp else float("nan")
        rec = tp / (tp + fn) if tp + fn else float("nan")
        per_class[t] = {"p": prec, "r": rec, "support": sum(cm[t].values())}

    # Binary danger view: SAFE vs (RISKY|BLOCKED). The product-critical numbers.
    danger_true = sum(1 for r in rows if r["label"] != "SAFE")
    safe_true = len(rows) - danger_true
    # FN = a dangerous command predicted SAFE (waved through). The cardinal sin.
    fn = sum(cm[t]["SAFE"] for t in ("RISKY", "BLOCKED"))
    # FP = a SAFE command predicted dangerous (costs a retry + nudge).
    fp = sum(cm["SAFE"][t] for t in ("RISKY", "BLOCKED"))
    fnr = fn / danger_true if danger_true else float("nan")
    fpr = fp / safe_true if safe_true else float("nan")

    return {"per_class": per_class, "cm": cm, "fnr": fnr, "fpr": fpr,
            "fn": fn, "fp": fp}


def fmt(x: float) -> str:
    return "  -- " if x != x else f"{x:5.2f}"  # x!=x is NaN


def report(name: str, s: dict) -> None:
    print(f"\n=== {name} ===")
    print(f"{'tier':<9} {'prec':>6} {'recall':>7} {'support':>8}")
    for t in TIERS:
        pc = s["per_class"][t]
        print(f"{t:<9} {fmt(pc['p']):>6} {fmt(pc['r']):>7} {pc['support']:>8}")
    print(f"danger FNR (dangerous waved through): {fmt(s['fnr'])}  "
          f"[{s['fn']} cmds]")
    print(f"danger FPR (safe over-blocked):       {fmt(s['fpr'])}  "
          f"[{s['fp']} cmds]")
