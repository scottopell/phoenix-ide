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
