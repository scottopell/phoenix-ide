# /// script
# requires-python = ">=3.11"
# dependencies = ["scikit-learn"]
# ///
"""Rung 2 of the baseline ladder — token n-gram + logistic regression.

The "a neural model must beat this" bar. Features a word(1-2)-gram +
char_wb(3-5)-gram union (char grams matter for shell syntax like `-rf`,
`/dev/`, `| sh` that word tokenizers shred), fed to a balanced multinomial
logistic regression. Trains on the synthetic corpus, scores on the frozen seed
set through evallib's metric — the identical code rungs 0/1 climb.

    uv run model_logreg.py            # load pickle if present, else train
    uv run model_logreg.py --retrain  # always refit deterministically
"""

from __future__ import annotations

import argparse
import pickle
import sys
from pathlib import Path

from sklearn.feature_extraction.text import TfidfVectorizer
from sklearn.linear_model import LogisticRegression
from sklearn.pipeline import FeatureUnion, Pipeline

import evallib

HERE = Path(__file__).parent
TRAIN = HERE / "data" / "train_synth.jsonl"
EVAL = HERE / "data" / "eval_seed.jsonl"
PICKLE = HERE / "model_logreg.pkl"
RANDOM_STATE = 0


def build_pipeline() -> Pipeline:
    # Word grams catch token-level shapes (`git push`, `--force`); char_wb grams
    # catch sub-token shell syntax (`-rf`, `/dev/`, `|sh`) that whitespace
    # tokenization would lose. Union both into one feature space.
    features = FeatureUnion([
        ("word", TfidfVectorizer(
            analyzer="word", ngram_range=(1, 2),
            token_pattern=r"(?u)\S+", min_df=1, sublinear_tf=True)),
        ("char", TfidfVectorizer(
            analyzer="char_wb", ngram_range=(3, 5),
            min_df=1, sublinear_tf=True)),
    ])
    clf = LogisticRegression(
        max_iter=2000, class_weight="balanced",
        C=4.0, random_state=RANDOM_STATE)
    return Pipeline([("feats", features), ("clf", clf)])


def train() -> Pipeline:
    rows = evallib.load_jsonl(TRAIN)
    X = [r["cmd"] for r in rows]
    y = [r["label"] for r in rows]
    pipe = build_pipeline()
    pipe.fit(X, y)
    with PICKLE.open("wb") as f:
        pickle.dump(pipe, f)
    return pipe


def load_or_train(retrain: bool) -> Pipeline:
    if not retrain and PICKLE.exists():
        with PICKLE.open("rb") as f:
            return pickle.load(f)
    return train()


def print_confusion(s: dict) -> None:
    cm = s["cm"]
    print("\nconfusion (rows = true, cols = pred):")
    header = " " * 10 + "".join(f"{t:>9}" for t in evallib.TIERS)
    print(header)
    for t in evallib.TIERS:
        cells = "".join(f"{cm[t][p]:>9}" for p in evallib.TIERS)
        print(f"{t:<10}{cells}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--retrain", action="store_true",
                    help="refit from the corpus even if a pickle exists")
    ap.add_argument("--data", default=str(EVAL),
                    help="JSONL eval set with {cmd,label} rows")
    args = ap.parse_args()

    pipe = load_or_train(args.retrain)

    def classify(cmd: str) -> str:
        return str(pipe.predict([cmd])[0])

    rows = evallib.load_jsonl(args.data, base=HERE)
    s = evallib.score(classify, rows)

    evallib.report("rung2-logreg", s)
    print_confusion(s)

    rung1_fnr = 0.74
    delta = rung1_fnr - s["fnr"]
    verdict = "BEATS" if s["fnr"] < rung1_fnr else "DOES NOT BEAT"
    print(f"\nrung-2 danger FNR {s['fnr']:.2f} vs rung1 {rung1_fnr:.2f} "
          f"-> {verdict} rung1 (Δ {delta:+.2f}); FPR {s['fpr']:.2f}")
    return 0


# Module-level classifier for callers that import this as a library / register
# it in eval.py's BASELINES. Lazily trains-or-loads on first use.
_PIPE: Pipeline | None = None


def classify(cmd: str) -> str:
    global _PIPE
    if _PIPE is None:
        _PIPE = load_or_train(retrain=False)
    return str(_PIPE.predict([cmd])[0])


if __name__ == "__main__":
    sys.exit(main())
