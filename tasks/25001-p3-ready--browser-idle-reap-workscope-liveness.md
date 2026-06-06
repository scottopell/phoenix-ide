Gate browser idle-reaping on work-scope liveness; keep the timer as a backstop.

PROBLEM
Browser idle cleanup is purely timer-driven and work-action-blind. A
background task (crates/phoenix-tools/src/browser/session.rs:cleanup_idle_sessions,
~1091) reaps any session whose last_activity is older than IDLE_TIMEOUT
(30 min, session.rs:69; checked every CLEANUP_INTERVAL=60s, session.rs:72).
last_activity only resets on a browser tool-call guard drop
(BrowserSessionGuard::drop, ~777). So a browser session is force-closed
after 30 min of no browser tool calls EVEN IF its work scope still owns a
non-terminal conversation that is open in the UI.

Consequence: live page state / open tabs / console buffer lost; a live
browser-view (REQ-BT-018) watcher sees Chrome die under them. Cookies
survive (on-disk profile /tmp/phoenix-chrome-{hash}) and relaunch is lazy,
so the cost is bounded -- but the live-view-dies-while-watching case is a
real UX wrinkle.

DESIRED BEHAVIOR
Do not reap a browser session while its WorkScope still owns a non-terminal
conversation (and/or while a live viewer Arc is attached). Keep the 30-min
timer as a backstop for genuinely abandoned scopes. This makes idle cleanup
WorkScope-liveness-aware, consistent with the cascade path which already
respects inheritor scope (cascade_browser_on_delete, session.rs:796).

SCOPE
Browser-only follow-up; independent of the bash WorkScope re-key. Spec
touch: specs/browser-tool/ (REQ-BT-010 implicit session model + the
WS reqs). Planned to be taken on AFTER the work-scope UI spec + the bash
re-key implementation, at the end of the sequence.
EOF2
