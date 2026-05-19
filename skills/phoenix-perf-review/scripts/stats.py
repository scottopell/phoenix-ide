#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Reduce raw scenario RunSample arrays to significance stats.

The harness MUST NOT compute stats (REQ-BT-019.5 / NG-STATS). This is the
caller-owned reduction: mean, sample stddev, % change, Welch's t-test p.
Input is produced by phoenix-perf-shared/scripts/run-scenario.

Usage:
    stats.py BASELINE.json OPTIMIZED.json

Each file is a JSON array of RunSample objects (raw, one per post-warmup run).
Key names may drift (harness changes, optional CDP add-on), so each metric is
extracted by a list of candidate paths; the first that resolves wins.
Unknown/absent metrics are skipped, never invented. Add a path here when the
harness schema changes — this file, not the harness, is where stats live.
"""

import json
import math
import sys

# metric label -> ordered candidate key paths into a RunSample.
# A path is a tuple; "*sum" means: value is a list/dict of per-component
# entries, sum their `actual_duration`/`actualDuration`/numeric values.
METRICS = {
    # primary keys = what phoenix-perf-shared/scripts/run-scenario emits.
    # extra paths = tolerance for the optional CDP add-on / schema drift.
    "react_commits": [
        ("react_commit_count",),   # agent-browser transport key
        ("react_commits",),        # browser_profile transport key
        ("react", "commit_count"),
        ("commits", "*len"),
    ],
    "react_actual_ms": [
        ("react_commit_ms",),      # agent-browser transport key
        ("react_actual_ms",),      # browser_profile transport key
        ("react", "actual_duration_ms"),
        ("commits", "*sum:actualDuration"),
    ],
    "script_ms": [
        ("script_ms",),
        ("macro_delta", "ScriptDuration"),
        ("ScriptDuration_delta",),
    ],
    "wall_ms": [
        ("wall_ms",),
    ],
    "js_heap_used_mib": [
        ("js_heap_used",),
        ("macro_after", "JSHeapUsedSize"),
        ("JSHeapUsedSize",),
    ],
    "long_tasks": [
        ("long_tasks",),
        ("long_task_count",),
    ],
    "dom_nodes": [
        ("dom_nodes",),
        ("Nodes",),
    ],
}

HEAP_KEYS = {"js_heap_used_mib"}  # bytes -> MiB if value looks like bytes


def _resolve(sample, path):
    cur = sample
    for i, key in enumerate(path):
        if isinstance(key, str) and key.startswith("*"):
            if key == "*len":
                return float(len(cur)) if cur is not None else None
            if key.startswith("*sum:"):
                field = key[5:]
                if not isinstance(cur, (list, tuple)):
                    return None
                total = 0.0
                for e in cur:
                    v = e.get(field) if isinstance(e, dict) else None
                    if isinstance(v, (int, float)):
                        total += v
                return total
            return None
        if isinstance(cur, dict) and key in cur:
            cur = cur[key]
        else:
            return None
    return cur if isinstance(cur, (int, float)) else None


def extract(samples, label):
    for path in METRICS[label]:
        vals = [_resolve(s, path) for s in samples]
        present = sum(v is not None for v in vals)
        if present == len(vals) and vals:
            if label in HEAP_KEYS and any(v > 1 << 20 for v in vals):
                vals = [v / (1 << 20) for v in vals]
            return vals
        if present != 0:
            path_s = ".".join(path)
            raise ValueError(
                f"metric {label!r} is partially present at path {path_s!r}: "
                f"{present}/{len(vals)} samples resolved; refusing to skip incomplete instrumentation"
            )
    return None


def mean(xs):
    return sum(xs) / len(xs)


def variance(xs):
    if len(xs) < 2:
        return 0.0
    m = mean(xs)
    return sum((x - m) ** 2 for x in xs) / (len(xs) - 1)


def _betacf(a, b, x):
    qab, qap, qam = a + b, a + 1.0, a - 1.0
    c, d = 1.0, 1.0 - qab * x / qap
    d = 1e-30 if abs(d) < 1e-30 else d
    d = 1.0 / d
    h = d
    for m in range(1, 200):
        m2 = 2 * m
        aa = m * (b - m) * x / ((qam + m2) * (a + m2))
        d = 1.0 + aa * d
        d = 1e-30 if abs(d) < 1e-30 else d
        c = 1.0 + aa / c
        c = 1e-30 if abs(c) < 1e-30 else c
        d = 1.0 / d
        h *= d * c
        aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2))
        d = 1.0 + aa * d
        d = 1e-30 if abs(d) < 1e-30 else d
        c = 1.0 + aa / c
        c = 1e-30 if abs(c) < 1e-30 else c
        d = 1.0 / d
        delta = d * c
        h *= delta
        if abs(delta - 1.0) < 3e-7:
            break
    return h


def _betai(a, b, x):
    if x <= 0.0:
        return 0.0
    if x >= 1.0:
        return 1.0
    lbeta = math.lgamma(a + b) - math.lgamma(a) - math.lgamma(b)
    bt = math.exp(lbeta + a * math.log(x) + b * math.log(1.0 - x))
    if x < (a + 1.0) / (a + b + 2.0):
        return bt * _betacf(a, b, x) / a
    return 1.0 - bt * _betacf(b, a, 1.0 - x) / b


def welch_p(a, b):
    """Two-sided Welch's t-test p-value. Exact via regularized incomplete beta."""
    na, nb = len(a), len(b)
    if na < 2 or nb < 2:
        return float("nan")
    va, vb = variance(a), variance(b)
    sa, sb = va / na, vb / nb
    denom = sa + sb
    if denom == 0:
        return 1.0 if mean(a) == mean(b) else 0.0
    t = (mean(a) - mean(b)) / math.sqrt(denom)
    df = denom**2 / ((sa**2 / (na - 1)) + (sb**2 / (nb - 1)))
    return _betai(0.5 * df, 0.5, df / (df + t * t))


def fmt(label, base, opt):
    bm, om = mean(base), mean(opt)
    bsd, osd = math.sqrt(variance(base)), math.sqrt(variance(opt))
    pct = ((om - bm) / bm * 100.0) if bm else float("nan")
    p = welch_p(base, opt)
    return (
        f"{label:28s} {bm:12.4g} ± {bsd:<10.3g} -> "
        f"{om:12.4g} ± {osd:<10.3g}  {pct:+7.2f}%  p={p:.4g}"
    )


def main():
    if len(sys.argv) != 3:
        sys.exit(f"usage: {sys.argv[0]} BASELINE.json OPTIMIZED.json")
    with open(sys.argv[1]) as f:
        base_s = json.load(f)
    with open(sys.argv[2]) as f:
        opt_s = json.load(f)
    if not isinstance(base_s, list) or not isinstance(opt_s, list):
        sys.exit("inputs must be JSON arrays of raw RunSample objects")
    print(f"baseline n={len(base_s)}  optimized n={len(opt_s)}")
    print(f"{'metric':28s} {'baseline':>25s}    {'optimized':>25s}   change   welch")
    print("-" * 100)
    any_metric = False
    for label in METRICS:
        b = extract(base_s, label)
        o = extract(opt_s, label)
        if b is None or o is None:
            print(f"{label:28s} (absent in samples — skipped, not invented)")
            continue
        any_metric = True
        print(fmt(label, b, o))
    if not any_metric:
        sys.exit("\nERROR: no known metric resolved — schema drifted; "
                 "add the new key path to METRICS in this file.")
    print("\nGate: a metric is a win only if it clears its threshold "
          "(see phoenix-perf README) AND p < 0.05.")


if __name__ == "__main__":
    main()
