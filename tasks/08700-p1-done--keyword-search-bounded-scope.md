keyword_search is catastrophically slow (minutes, per broad term) when the conversation's working dir is a large tree — e.g. an intentional "all-known-code" root like ~/go/src/github.com/DataDog (49G across 28 sibling repos, no .git of its own). It is NOT deadlocked; it makes forward progress but scans the whole tree with `rg -C 10` and full buffering, once per search term.

Design intent to preserve: rooting a conversation at a broad multi-repo dir is a deliberate, supported workflow (agents answer questions from any repo). Do NOT refuse or auto-narrow that cwd. The job is to make searching it cheap and bounded, not forbidden.

Two seams:

(a) Scope resolution / root floor — LIGHT TOUCH.
- Keep find_search_root() behavior (walk up to git root; else fall back to cwd). This already does the right thing: whole-repo coverage when inside one repo, and respects the broad container cwd otherwise.
- Add a floor: never allow the resolved search root to be the filesystem root `/` (defense-in-depth; REQ-PROJ-000 already floors conversation cwd, this is belt-and-suspenders). Error clearly if it ever happens.
- Log the resolved search root at info/debug so an oversized scope is visible.

(b) Bounded, cheap search — THE ACTUAL FIX.
Root cause: the "reject overly broad terms" probe (keyword_search.rs:255-279) measures breadth by running the FULL `rg -C 10` scan and buffering the entire output into memory, THEN checking len > 64KB. It pays the full cost of the expensive query it is trying to avoid, per term. Over 49G that is minutes per broad term.
- Replace the probe with an early-exit match count: `rg -c -i -e <term>`, stream stdout, accumulate per-file counts, and the instant the running total crosses BROAD_TERM_MATCH_LIMIT, kill rg and mark the term "too broad". This is O(limit) work regardless of tree size — a broad term is rejected after finding ~limit matches, not after walking 49G. A narrow term completes quickly on its own.
- Replace the combined-phase peel loop (255-301) with a single `rg -C 10` scan over the usable terms whose stdout is read with a STREAMING byte cap: stop/kill rg once output exceeds MAX_COMBINED_RESULTS and append a truncation marker telling the agent the scope was large and to narrow terms. This is the always-on ceiling that fires even inside a legitimate single repo (per decision).
- Preserve cooperative cancellation (REQ-BED-005): every rg child must still be raced against ctx.cancel and killed+reaped on cancel, matching the existing select!/kill_on_drop pattern.

Thresholds (consts, tune): BROAD_TERM_MATCH_LIMIT ~ a few hundred matches; MAX_COMBINED_RESULTS stays 128KB. MAX_TERM_RESULTS (the old 64KB byte gate) is superseded by the match-count gate.

Spec: specs/keyword_search/ (requirements.md is normative). REQ-KWS-002 (search scope) stays true. Update executive.md to reflect the bounded/early-exit behavior and the always-on combined cap; add rationale to requirements if the always-on ceiling is a new normative behavior. design.md is legacy v1 — do not extend it.

Acceptance: from a conversation rooted at a 49G/28-repo dir, a keyword_search with a mix of broad ("controller", "reconcile") and narrow terms returns in seconds, not minutes; broad terms are dropped cheaply; combined output is capped with a clear truncation signal; cancellation still kills rg children promptly; cargo test passes.
