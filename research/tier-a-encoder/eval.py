# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Tier A eval harness — the frozen ratchet every classifier climbs.

Scores classifiers on a labeled command set with PER-CLASS precision/recall.
Aggregate accuracy is deliberately not reported: under the real-world class
prior (overwhelmingly SAFE) it is a vanity metric that hides the only failures
that matter — a dangerous command waved through (false negative on BLOCKED).

Add a model by writing a `classify(cmd: str) -> str` callable and registering it
in BASELINES. The data and the metric do not change; that is the point.

    uv run eval.py
    uv run eval.py --data data/eval_seed.jsonl
"""

from __future__ import annotations

import argparse
import re
import shlex
import sys
from collections import defaultdict
from pathlib import Path

from evallib import TIERS, fmt, load_jsonl, report, score  # noqa: F401


# ---------------------------------------------------------------------------
# Rung 0 — majority class. The floor that exposes the imbalance lie.
# ---------------------------------------------------------------------------
def majority(cmd: str) -> str:
    return "SAFE"


# ---------------------------------------------------------------------------
# Rung 1 — bash_check rules-as-classifier.
#
# Mirrors crates/phoenix-tools/src/bash_check.rs: blind `git add`, `git push
# --force` (but NOT --force-with-lease), and dangerous `rm -rf`. Everything the
# rules don't recognize falls through to SAFE — exactly the deterministic Layer
# 0 behaviour. The gap between this rung's recall and 1.0 is precisely what the
# trained encoder exists to close.
#
# This is a string-level re-expression for eval convenience; the shipped Layer 0
# is AST-based (brush_parser). Tokenization differences on exotic inputs are
# acceptable here — this rung is a baseline, not the enforcement path.
# ---------------------------------------------------------------------------
_BLIND_GIT_ADD = {"-A", "--all", ".", "*"}
_RM_CRITICAL = re.compile(r"(^|/|\s)(/|~|\$HOME)(/\*?)?\s*$")


def _tokens(cmd: str) -> list[str]:
    try:
        return shlex.split(cmd)
    except ValueError:
        return cmd.split()


def bash_check_rules(cmd: str) -> str:
    # split on shell separators so `a && git push --force` is still caught
    for seg in re.split(r"&&|\|\||;|\|", cmd):
        toks = _tokens(seg.strip())
        if not toks:
            continue
        # strip a leading sudo
        if toks[0] == "sudo":
            toks = toks[1:]
        if len(toks) < 2:
            continue
        if toks[0] == "git" and toks[1] == "add":
            if any(a in _BLIND_GIT_ADD for a in toks[2:]):
                return "BLOCKED"
        if toks[0] == "git" and toks[1] == "push":
            args = toks[2:]
            if any(a.startswith("--force-with-lease") for a in args):
                continue
            if any(a == "--force" or a == "-f" for a in args):
                return "BLOCKED"
        if toks[0] == "rm":
            flags = "".join(a for a in toks if a.startswith("-"))
            recursive = "r" in flags or "--recursive" in toks
            force = "f" in flags or "--force" in toks
            if recursive and force:
                paths = " ".join(a for a in toks[1:] if not a.startswith("-"))
                if _RM_CRITICAL.search(" " + paths):
                    return "BLOCKED"
    return "SAFE"


BASELINES = {
    "rung0-majority": majority,
    "rung1-bash_check": bash_check_rules,
}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default="data/eval_seed.jsonl",
                    help="JSONL with {cmd,label} rows")
    args = ap.parse_args()

    rows = load_jsonl(args.data, base=Path(__file__).parent)

    dist = defaultdict(int)
    for r in rows:
        dist[r["label"]] += 1
    print(f"eval set: {len(rows)} commands  "
          f"({', '.join(f'{t}={dist[t]}' for t in TIERS)})")

    for name, fn in BASELINES.items():
        report(name, score(fn, rows))

    print("\nratchet rule: a new classifier ships only if it lowers danger FNR "
          "without raising FPR past target. Add it to BASELINES and re-run.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
