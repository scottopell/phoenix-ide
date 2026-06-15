# `wait_for_resize` silently discards pre-resize data frames (spec says treat-normally + warn)

## Problem

`terminal_ws.rs::wait_for_resize` blocks on the initial WebSocket handshake,
reading frames until it sees the first valid `0x01` resize frame. Any `0x00`
(PTY data / keystroke) frame that arrives *before* the resize is silently
dropped — the loop ignores everything that isn't a resize, with no log and no
handling.

The terminal spec contradicts this. `specs/terminal/terminal.allium`,
`InitialResizeSent` `@guidance`:

> "If the client sends a data frame before a resize frame, treat the data
>  frame normally and log a warning; the shell will have started at its PTY
>  default size (80x24)."

So the contract is "handle + warn," and the code does "discard, silently."

## Why this is latent, not user-facing today

The browser client (`TerminalPanel.tsx`) cannot trigger the discard. Per the
WHATWG WebSocket algorithm, the "connection established" task sets
`readyState = OPEN` and then synchronously fires the `open` event in the same
task. `onopen` sends the resize frame as its first action, and `onData` only
sends when `readyState === OPEN` — i.e. after `onopen` has already run and
queued the resize. TCP preserves order, so the server always sees the resize
before any data frame. The discard branch is effectively dead defensive code
for the current UI.

It becomes reachable for any *non-conforming* client: a future native/CLI
client, a test harness that writes input before resizing, or an intermediary
that reorders frames. In those cases input is lost at connect with zero
diagnostic — exactly the silent-drop class we just hardened against on the
client side (see the outbound-input buffer in `TerminalPanel.tsx`).

## Decision needed (resolve before implementing)

Pick one and make code + spec agree:

- **Align code to spec**: buffer pre-resize data frames during the handshake,
  spawn the PTY at the default 80x24, replay the buffered input once the relay
  starts, and `tracing::warn!` that a data frame preceded the resize. Matches
  `InitialResizeSent` as written.
- **Make discard the contract**: if treat-normally is undesirable (e.g. we
  prefer to guarantee the PTY is correctly sized before any input reaches the
  shell), keep dropping but `tracing::warn!` on discard so it isn't silent, and
  rewrite the `InitialResizeSent` `@guidance` to document discard-until-resize
  as the intended behavior.

Either way the silent path must go.

## Acceptance

- `wait_for_resize` no longer drops a pre-resize data frame without at least a
  `tracing::warn!`.
- `specs/terminal/terminal.allium` `InitialResizeSent` and the code agree on
  the pre-resize-data contract.
- A test exercises the pre-resize-data path (feed a `0x00` frame before the
  `0x01` resize) and asserts the chosen behavior.

## Context

Distinct from task 27105 (reclaim 409-vs-silent-reclaim). Surfaced while
diagnosing an intermittent dropped-keystroke report on a freshly-opened
terminal; the real connect-time drop was client-side (input typed during the
WS CONNECTING window), fixed by buffering outbound input in `TerminalPanel.tsx`.
This task tracks the server-side latent twin.
