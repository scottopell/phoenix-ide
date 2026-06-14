# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Synthesize a labeled shell-command TRAINING corpus for the Tier A risk tiers.

Distinct from the frozen eval seed (`data/eval_seed.jsonl`): this produces the
*training* half. Per-class template families with randomized slots (paths,
filenames, branches, URLs, flag orderings, sudo prefixes, harmless chaining)
give the corpus lexical diversity instead of N copies of one anchor string.

The label is determined by the **dangerous core**, per labels.md:
  - Judge the action alone (rule 1). A harmless `cd foo && <core>` prefix does
    not change the label of <core>.
  - Reversibility, not intent, is the axis (rule 2).
  - Scope widens severity (rule 3): `rm -rf ./build` is RISKY; `rm -rf /` BLOCKED.
  - Piping untrusted bytes to an interpreter is BLOCKED (rule 4).
  - Device/filesystem-level writes are BLOCKED (rule 5).

Deterministic: fixed RNG seed => byte-identical corpus across runs.

    uv run gen_corpus.py
    uv run gen_corpus.py --seed 7 --n 800
"""

from __future__ import annotations

import argparse
import json
import random
from collections import defaultdict
from pathlib import Path

DEFAULT_SEED = 1729
DEFAULT_N = 1200

TIERS = ["SAFE", "RISKY", "BLOCKED"]

# ---------------------------------------------------------------------------
# Slot vocabularies — varied so the corpus is lexically diverse.
# ---------------------------------------------------------------------------
FILES = [
    "main.rs", "lib.rs", "config.toml", "Cargo.toml", "app.log", "server.log",
    "data.db", "index.html", "styles.css", "package.json", "README.md",
    "notes.txt", "schema.sql", "mod.rs", "handler.go", "utils.py", "test.py",
    "Makefile", "Dockerfile", ".env.example", "report.csv", "build.rs",
]
SRC_FILES = [
    "src/main.rs", "src/lib.rs", "src/api/wire.rs", "crates/core/src/db.rs",
    "ui/src/App.tsx", "ui/src/index.ts", "cmd/server/main.go", "pkg/util.py",
    "internal/config.go", "tests/integration.rs", "app/models.py",
]
DIRS = [
    "build", "dist", "target", "node_modules", "tmp", "cache", ".cache",
    "out", "coverage", "logs", "vendor", "__pycache__", "bin", "obj",
]
REL_DIRS = ["./" + d for d in DIRS] + [d + "/" for d in DIRS]
BRANCHES = [
    "feature/x", "feature/login", "feature/auth-rework", "bugfix/crash",
    "hotfix/leak", "wip/experiment", "old-branch", "release/v2", "dev",
    "topic/refactor", "stale/2024", "spike/perf",
]
REMOTES = ["origin", "upstream", "fork", "mirror"]
PUSH_BRANCHES = ["main", "master", "develop", "release", "trunk"]
STATUS_URLS = [
    "https://api.example.com/status", "https://api.github.com/rate_limit",
    "http://localhost:8042/health", "https://registry.npmjs.org/-/ping",
    "https://example.com/api/v1/health", "https://status.example.io/healthz",
]
INSTALL_URLS = [
    "https://get.example.com/install.sh", "https://x.io/i.sh",
    "https://sh.rustup.rs", "https://deb.nodesource.com/setup_20.x",
    "https://raw.githubusercontent.com/acme/tool/main/install.sh",
    "https://install.example.org/bootstrap",
]
PIDS = ["1234", "4242", "9090", "27183", "555", "31337", "8080"]
PROCS = ["node", "python", "phoenix", "java", "ruby", "cargo", "rustc", "vite"]
SED_SUBS = [
    "s/foo/bar/g", "s/localhost/127.0.0.1/", "s/debug/info/g",
    "s|/old/path|/new/path|", "s/v1/v2/g", "/^#/d",
]
HARMLESS_PREFIXES = [
    "cd {dir} &&", "cd {dir} ;", "echo start &&", "set -e &&",
]
HARMLESS_DIRS = ["repo", "project", "workspace", "app", "src", "/tmp/work", "build"]


def maybe_sudo(rng: random.Random, p: float = 0.25) -> str:
    return "sudo " if rng.random() < p else ""


def maybe_prefix(rng: random.Random, p: float = 0.18) -> str:
    """A harmless leading command joined by && or ; — must not change the label."""
    if rng.random() >= p:
        return ""
    tmpl = rng.choice(HARMLESS_PREFIXES)
    return tmpl.format(dir=rng.choice(HARMLESS_DIRS)) + " "


def rm_flags(rng: random.Random) -> str:
    """A recursive+force rm flag spelling: -rf, -fr, -r -f, --recursive --force, ..."""
    return rng.choice([
        "-rf", "-fr", "-Rf", "-rf", "-r -f", "-f -r",
        "--recursive --force", "-rf --no-preserve-root",
    ])


# ---------------------------------------------------------------------------
# SAFE generators — read-only or no significant state change.
# ---------------------------------------------------------------------------
def gen_safe(rng: random.Random) -> str:
    f = rng.choice(SRC_FILES + FILES)
    d = rng.choice(DIRS)
    builders = [
        lambda: rng.choice(["ls", "ls -la", "ls -l", "ls -lah", f"ls {d}/"]),
        lambda: rng.choice([f"cat {f}", f"cat {rng.choice(SRC_FILES)}"]),
        lambda: rng.choice([
            f"grep -rn {rng.choice(['TODO', 'FIXME', 'fn main', 'import', 'error'])} {rng.choice(['.', 'src/', 'crates/'])}",
            f"grep {rng.choice(['-i', '-c', '-n', '-E'])} {rng.choice(['warn', 'panic', 'todo'])} {f}",
        ]),
        lambda: f"find {rng.choice(['.', 'src', './crates'])} -name '*.{rng.choice(['rs', 'py', 'go', 'ts', 'log'])}' -type f",
        lambda: rng.choice(["git status", "git status -s", "git status --short"]),
        lambda: rng.choice([f"git diff {rng.choice(['', 'HEAD~1', '--staged', '--stat'])}".strip(), f"git diff {f}"]),
        lambda: rng.choice([f"git log {rng.choice(['--oneline -20', '-5', '--stat', '--graph --oneline'])}"]),
        lambda: rng.choice(["cargo build", "cargo build --release", "cargo check", "cargo build -p core"]),
        lambda: rng.choice(["cargo test", f"cargo test {rng.choice(['state_machine', 'db', 'runtime', '-- --nocapture'])}"]),
        lambda: f"echo {rng.choice(['hello world', 'done', 'building...', '$PATH', 'ok'])}",
        lambda: f"head -{rng.choice([10, 20, 50, 100])} {f}",
        lambda: f"wc -l {rng.choice([f, 'src/*.rs', '*.py'])}",
        lambda: f"mkdir -p {rng.choice(['build/out', 'tmp/work', 'dist', d + '/sub'])}",
        lambda: f"cp {f} {f}.bak",
        lambda: f"git add {rng.choice(SRC_FILES + FILES)}",  # explicit file: SAFE
        lambda: f"git commit -m '{rng.choice(['fix bug', 'add tests', 'refactor', 'update deps', 'wip'])}'",
        lambda: f"git checkout -b {rng.choice(BRANCHES)}",
        lambda: f"curl -s {rng.choice(STATUS_URLS)}",
        lambda: f"curl {rng.choice(['-sS', '-fsSL', '-I'])} {rng.choice(STATUS_URLS)}",
        lambda: rng.choice(["pwd", "df -h", "whoami", "date", "uname -a", "env | sort"]),
        lambda: f"ps aux | grep {rng.choice(PROCS)}",
        lambda: f"git push --force-with-lease {rng.choice(REMOTES)} {rng.choice(BRANCHES)}".strip(),
        lambda: "git push --force-with-lease",
        lambda: f"git push --force-with-lease={rng.choice(REMOTES)}/{rng.choice(PUSH_BRANCHES)}",
    ]
    return rng.choice(builders)()


# ---------------------------------------------------------------------------
# RISKY generators — may irreversibly alter LOCAL state; scope is non-critical.
# ---------------------------------------------------------------------------
def gen_risky(rng: random.Random) -> str:
    f = rng.choice(FILES)
    builders = [
        lambda: f"git reset --hard {rng.choice(['HEAD~1', 'HEAD~3', 'origin/main', 'HEAD', f'origin/{rng.choice(PUSH_BRANCHES)}'])}",
        lambda: rng.choice(["git clean -fd", "git clean -fdx", "git clean -f", "git clean -fdX"]),
        lambda: f"git checkout -- {rng.choice(['.', f, rng.choice(SRC_FILES)])}",
        # rm of NON-critical relative paths => RISKY (scope rule keeps it below BLOCKED)
        lambda: f"rm -rf {rng.choice(REL_DIRS)}",
        lambda: f"rm {rng.choice(['-f', ''])} {rng.choice(['*.log', '*.tmp', '*.bak', f, './' + f])}".replace("  ", " ").strip(),
        lambda: f"rm {f}",
        lambda: rng.choice(["docker system prune -af", "docker system prune -f", "docker volume prune -f", "docker image prune -af"]),
        lambda: "docker rmi -f $(docker images -q)",
        lambda: f"kill -9 {rng.choice(PIDS)}",
        lambda: f"pkill {rng.choice(['-9', '-f', ''])} {rng.choice(PROCS)}".replace("  ", " "),
        lambda: f"truncate -s 0 {rng.choice(['app.log', 'server.log', f])}",
        lambda: f"git branch -D {rng.choice(BRANCHES)}",
        lambda: rng.choice(["git stash drop", "git stash clear", f"git stash drop stash@{{{rng.choice([0,1,2])}}}"]),
        lambda: rng.choice(["npm publish", "npm publish --access public", "cargo publish"]),
        # plain push (no force) => RISKY
        lambda: rng.choice(["git push", f"git push {rng.choice(REMOTES)} {rng.choice(BRANCHES)}", f"git push -u {rng.choice(REMOTES)} {rng.choice(BRANCHES)}"]),
        lambda: f"mv {f} {rng.choice(['/tmp/', '/tmp/' + f, 'backup/' + f])}",
        lambda: f"chmod -R {rng.choice(['777', '755', '700'])} {rng.choice(REL_DIRS)}",
        lambda: f"sed -i {rng.choice(['', '.bak'])} '{rng.choice(SED_SUBS)}' {f}".replace("  ", " "),
    ]
    return rng.choice(builders)()


# ---------------------------------------------------------------------------
# BLOCKED generators — will irreversibly alter state regardless of context.
# ---------------------------------------------------------------------------
def gen_blocked(rng: random.Random) -> str:
    builders = [
        # rm -rf on critical paths (scope rule pushes to BLOCKED); flag reorderings + sudo
        lambda: f"{maybe_sudo(rng)}rm {rm_flags(rng)} {rng.choice(['/', '~', '$HOME', '/*', '~/', '$HOME/'])}",
        # git push --force / -f (but NOT --force-with-lease — that's SAFE)
        lambda: f"git push {rng.choice(['--force', '-f'])} {rng.choice(['', rng.choice(REMOTES) + ' ' + rng.choice(PUSH_BRANCHES), rng.choice(REMOTES)])}".strip(),
        lambda: f"git push {rng.choice(REMOTES)} {rng.choice(PUSH_BRANCHES)} {rng.choice(['--force', '-f'])}",
        # blind git add
        lambda: f"git add {rng.choice(['-A', '--all', '.', './', '-A .', '*'])}",
        # curl/wget piped to interpreter
        lambda: f"{rng.choice(['curl -fsSL', 'curl -s', 'curl'])} {rng.choice(INSTALL_URLS)} | {rng.choice(['sh', 'bash', 'sudo bash', 'sudo sh'])}",
        lambda: f"wget {rng.choice(['-qO-', '-O-'])} {rng.choice(INSTALL_URLS)} | {rng.choice(['sh', 'bash'])}",
        # dd to a device
        lambda: f"{maybe_sudo(rng)}dd if={rng.choice(['/dev/zero', '/dev/urandom', 'disk.img'])} of=/dev/{rng.choice(['sda', 'sda1', 'nvme0n1', 'disk0'])}{rng.choice(['', ' bs=1M'])}",
        # mkfs
        lambda: f"{maybe_sudo(rng)}mkfs.{rng.choice(['ext4', 'xfs', 'btrfs'])} /dev/{rng.choice(['sda1', 'sdb1', 'nvme0n1p1'])}",
        # chmod -R 777 on root
        lambda: f"{maybe_sudo(rng)}chmod -R 777 {rng.choice(['/', '/*', '/usr', '/etc'])}",
        # fork bomb
        lambda: rng.choice([":(){ :|:& };:", ":(){ :|: & };:"]),
        # redirect to a device
        lambda: f"{maybe_sudo(rng)}echo {rng.choice(['1', '0', 'x', 'data'])} > /dev/{rng.choice(['sda', 'sda1', 'nvme0n1'])}",
    ]
    return rng.choice(builders)()


GENERATORS = {"SAFE": gen_safe, "RISKY": gen_risky, "BLOCKED": gen_blocked}


def load_eval_cmds(here: Path) -> set[str]:
    path = here / "data" / "eval_seed.jsonl"
    cmds: set[str] = set()
    for line in path.read_text().splitlines():
        line = line.strip()
        if line:
            cmds.add(json.loads(line)["cmd"])
    return cmds


def build(seed: int, n: int) -> tuple[list[dict], dict]:
    here = Path(__file__).parent
    eval_cmds = load_eval_cmds(here)
    rng = random.Random(seed)

    seen: set[str] = set(eval_cmds)  # exclude eval rows so train/test never overlap
    rows: list[dict] = []
    counts: dict[str, int] = defaultdict(int)

    for tier in TIERS:
        gen = GENERATORS[tier]
        attempts = 0
        # generous attempt budget so the template space is mined without infinite loop
        max_attempts = n * 200
        while counts[tier] < n and attempts < max_attempts:
            attempts += 1
            core = gen(rng)
            cmd = maybe_prefix(rng) + core  # prefix is harmless; label is the core's
            if cmd in seen:
                continue
            seen.add(cmd)
            rows.append({"cmd": cmd, "label": tier})
            counts[tier] += 1

    return rows, dict(counts)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--seed", type=int, default=DEFAULT_SEED)
    ap.add_argument("--n", type=int, default=DEFAULT_N,
                    help="target rows per class")
    args = ap.parse_args()

    rows, counts = build(args.seed, args.n)

    here = Path(__file__).parent
    out = here / "data" / "train_synth.jsonl"
    out.write_text("".join(json.dumps(r) + "\n" for r in rows))

    print(f"wrote {out}")
    print(f"class counts: " + ", ".join(f"{t}={counts.get(t, 0)}" for t in TIERS))
    print(f"total: {len(rows)}")
    short = [t for t in TIERS if counts.get(t, 0) < args.n]
    if short:
        print(f"note: under target for {short} — template space exhausted "
              f"after dedup (raise diversity or lower --n)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
