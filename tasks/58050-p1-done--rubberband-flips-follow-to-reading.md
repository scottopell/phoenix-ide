iOS rubber-band overscroll at the bottom silently flips follow → reading, then the unread chip appears while the user is pinned at the bottom and can never auto-clear.

Mechanism:

- `MessageList`'s native `scroll` handler classifies any `scrollTop` decrease as `upwardIntent` (strict `snapshot.scrollTop < previousTop`, no threshold, no overscroll guard).
- On iOS Safari, flinging to the bottom overscrolls (`scrollTop > scrollHeight - clientHeight`) then bounces back with several frames of *decreasing* scrollTop — after the finger is already lifted, so `gesture` is idle and nothing blocks `takeUserOwnership`. The machine enters `reading` while the user is visually pinned at the bottom.
- Throughout the bounce, VirtualTranscript's `store.pinned` stays true (`maxScrollTop - viewportTop <= 1` holds during overscroll), so no `viewportPinnedChanged` edge ever fires afterward — `confirmTailReturn` is unreachable (same edge-starvation as task 58049).
- Next `tailContentAdvanced` → blocked (reading) → `unread: true`: the chip appears while the user is at the bottom and stays until clicked. The machine also stops scheduling tail-follows, which is masked by VirtualTranscript's independent `wasPinned` auto-snap.

Fix direction: ignore `upwardIntent` when the snapshot is at/beyond the bottom (e.g. `scrollHeight - scrollTop - clientHeight <= PIN_TO_BOTTOM_THRESHOLD`, or at minimum `scrollTop >= maxScrollTop`, which covers the overscroll bounce). Clamping the snapshot's scrollTop into `[0, maxScrollTop]` before direction classification also fixes the symmetric top-overscroll misclassification.
