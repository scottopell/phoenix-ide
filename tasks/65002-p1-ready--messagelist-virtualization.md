Virtualize MessageList — the measured, reproduced cause of "slow to switch
into bigger conversations."

QUANTIFIED PROBLEM (phoenix-perf suite, scenario conversation-load,
fixture-turn-one = 47 large mock messages, N=12, agent-browser harness):
  react_commit_ms  median 600  (sd 59, CV ~10%)
  script_ms        median 363
  long_tasks       median 2     (>50ms main-thread stalls = visible jank)
  wall_ms          ~1100        (>1s switch — matches the user report)
Cause: MessageListBody (ui/src/components/MessageList.tsx) renders ALL
message subtrees (each AgentMessage = markdown + Prism syntax highlight +
tool blocks) with NO virtualization on conversation switch.

WHY THIS IS A TASK, NOT A HUNT ITERATION
Correct virtualization is feature-sized and regression-prone. The scroll
model it must not break:
 - REQ-CONV-013 scroll-position restore (localStorage; useLayoutEffect on
   mount; depends on full scrollHeight).
 - REQ-CONV-019 StreamingMessage sibling lives in #messages, outside
   MessageListBody, and must keep getting per-token updates.
 - Single ResizeObserver on #messages drives ALL auto-scroll; pinned-to-
   bottom math uses mainRef scrollHeight/scrollTop/clientHeight.
 - 'force'/'soft'/'none' scroll-trigger logic on new messages.
 - Variable, unknown message heights.

RECOMMENDED APPROACH (lower regression risk than DOM-windowing)
Render-virtualization preserving geometry: keep every message's wrapper in
layout at its real height; swap the EXPENSIVE subtree (AgentMessage
markdown/Prism) for a same-height lightweight placeholder when offscreen,
upgrade on scroll-in via IntersectionObserver. Geometry (scrollHeight,
ResizeObserver, scroll-restore, pinned math) stays byte-identical, so the
spec'd behaviors are untouched; only offscreen render cost drops.
Risk to design out: placeholder height accuracy (mismatch -> scroll jump
on swap). Need measured/cached per-message heights.

ACCEPTANCE (via the phoenix-perf suite — this is checklist item 1)
 - Recapture conversation-load baseline immediately before the change
   (baseline drifts ~10% run-to-run; never reuse the numbers above —
   they are the problem statement, not the A/B baseline).
 - react_commit_ms reduction >= the gate (>=10%) AND Welch p<0.05; expect
   far more (offscreen ~80% of 47 msgs).
 - 5-persona review: Conservative MUST verify no regression in scroll
   restore, auto-scroll/pinned, streaming updates, jump-to-newest — by
   behavior, not just metrics. A scroll-jump on placeholder swap is a
   REJECT regardless of the perf win.
 - Record in skills/phoenix-perf-hunt/resources/db.yaml.

Its own PR. Suite + harness already validated and on this branch.

---

## MEASURED 2026-05-16 — upside CONFIRMED, naive design REJECTED on correctness

A geometry-preserving render-virtualization attempt was implemented, measured
through the suite, and REJECTED. Full record:
skills/phoenix-perf-hunt/resources/db/MessageList-virtualize-render.yaml

PERF UPSIDE (real, huge, significant — the lever is correct):
  react_commit_ms  642 -> 243   (-62%, p=1.8e-14)
  script_ms        422 -> 226   (-46%, p=1.3e-4)
  dom_nodes        2813 -> 2509 (-11%)
Baseline re-measured on old code with a build-equivalent readiness predicate
(stash impl / measure / pop) so the A/B was valid.

WHY REJECTED — two QA-confirmed behavior regressions (perf cannot rescue):
  1. Switch no longer lands pinned-to-bottom: after switch, scrollTop ~25k,
     distFromBottom ~31k, newest row an unrendered spacer. User stops seeing
     the latest message on switch.
  2. estimate->measured height correction shifts scrollHeight ~5.5% =
     visible scroll jump.

A CORRECT DESIGN MUST SOLVE BY CONSTRUCTION (not patch):
  (a) Pinned-to-bottom on switch: seed visibility from the BOTTOM (render the
      last screenful real on mount), not "all rows start hidden, let
      IntersectionObserver flip them" — that fundamentally fights the
      ResizeObserver scroll-to-bottom (chicken/egg).
  (b) No estimate-driven jump: bottom-anchored layout, OR render-real until a
      measured height is cached, OR a measure-pass before virtualizing. Never
      lay out on a guessed height that later corrects under the user.
  (c) Validate REQ-CONV-013 scroll-restore against (b).
  (d) Re-use the suite: scenario conversation-load + the build-equivalent
      predicate already in place; baseline ~635ms react_commit_ms (CV 3%).

STATUS: stays p1/ready. Upside (~-62% commit) justifies the dedicated
redesign. This is checklist item 1's path — but only with (a)-(c) solved and
QA-2 behavior verification green.
