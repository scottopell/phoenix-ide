# Terminal Panel

## Scope

This spec governs the user-facing terminal experience inside a
conversation: opening a real shell, typing into it, knowing whether
it's alive, knowing what command is running and how the last one
ended, and recovering cleanly when something goes wrong.

The backend PTY session, WebSocket protocol, and OSC marker semantics
on the server side are owned by `specs/terminal/`. That spec
explicitly defers UI panel placement to the implementation; this
document fills that gap. See `specs/terminal-panel/executive.md` for
the full boundary, and `specs/terminal-panel/design.md` for the
implementation that delivers the requirements below.

## User Story

As a Phoenix user inside a conversation, I want a real terminal pane I
can open without leaving the page — my shell, my dotfiles, my
working directory. I want to be able to tell at a glance whether the
shell is alive, whether a command is running, and how the last one
ended, even when the pane is collapsed out of the way. When something
goes wrong, I want to know it went wrong and have an obvious next
step.

## Transparency Contract

The panel must let me confidently answer:

1. Is my terminal connected right now?
2. If not, can I get it back without losing context?
3. Is a command currently running? Which one? For how long?
4. What was the last command, and did it succeed?
5. What directory am I in?
6. Is shell integration enabled? If not, how do I enable it?
7. Did my output survive when I collapsed the panel away?
8. If I have this terminal open in another tab, will I be told before
   it gets taken over?

Each numbered question maps to one or more requirements below.

---

## Requirements

### REQ-TPANEL-001: Open and Use a Terminal in My Conversation

WHEN I open the terminal panel inside a conversation
THE SYSTEM SHALL connect me to that conversation's shell, starting in
its working directory, and let me type and see output as if I had
opened a real terminal locally

WHEN I type
THE SYSTEM SHALL forward keystrokes to my shell and render output as
they arrive

WHEN the panel resizes — window resize, layout change, collapse and
re-expand
THE SYSTEM SHALL keep the terminal usable: line wrap stays correct,
the shell agrees on the new geometry, and my place in the scrollback
is preserved

**Rationale:** The panel is not a simulated shell. It is the actual
PTY-backed shell from `specs/terminal/`, presented in the browser.
Every interactive program (vim, htop, fzf), readline, dotfile, and
ANSI behaviour works as it would in any native terminal. Resize is
load-bearing because the line-wrap experience breaks immediately when
the shell and the terminal disagree on column width.

---

### REQ-TPANEL-002: Know When the Terminal Has Disconnected, and Recover Without Losing Context

WHEN the terminal connection ends — the shell exited, the network
dropped, the backend restarted, or any other reason
THE SYSTEM SHALL show me clearly that the terminal is disconnected
(not silently retry, not pretend everything is fine)

WHEN I want to recover
THE SYSTEM SHALL provide an obvious one-click action to start a fresh
terminal in the same conversation

WHEN I trigger that action
THE SYSTEM SHALL spawn a new terminal in the same starting directory,
without disturbing the rest of my conversation state (messages,
breadcrumbs, file viewer, etc.)

THE SYSTEM SHALL NOT auto-retry the connection. Recovery is a thing I
do, on purpose.

**Rationale:** Auto-retry hides intermittent failures and produces
retry storms when the backend is genuinely unhealthy. An explicit
"reconnect" click keeps me in the loop and tells the system "I want
this back, please." The conversation is more than the terminal —
recovering the terminal must not throw away the rest of what I'm
doing.

---

### REQ-TPANEL-003: See at a Glance What's Running or What Just Finished

WHEN a command is currently executing
THE SYSTEM SHALL show its text and a live elapsed-time counter in the
panel header

WHEN a command has just finished
THE SYSTEM SHALL show whether it succeeded (✓), failed (✗), or had no
exit code reported (•), along with its text and either the exit code
or the duration

WHEN no command is running and none has just finished
THE SYSTEM SHALL show my current working directory in the header (see
REQ-TPANEL-004)

WHEN I have collapsed the panel
THE SYSTEM SHALL still render the header, so the live status is
visible at a glance without expanding

**Rationale:** The "at a glance" contract relies on my shell emitting
standard FinalTerm OSC 133 markers. When my shell does NOT emit them
(see REQ-TPANEL-006), the system shows a coarser "shell is producing
output" / "shell is quiet" indicator instead. The system never makes
up command outcomes that the shell did not report — a • (unknown
exit) is honest; a fabricated ✓ would not be.

---

### REQ-TPANEL-004: See My Current Working Directory in the Header

WHEN my shell reports its current directory via OSC 7 (zsh +
powerlevel10k, fish 3.6+, bash with the snippet from REQ-TPANEL-006)
THE SYSTEM SHALL show that live path in the panel header

WHEN my shell does not report cwd
THE SYSTEM SHALL show the conversation's starting directory as a
stable fallback

**Rationale:** With OSC 7, the header tracks `cd` in real time —
useful when the conversation's starting directory is a project root
but I'm working in a subdirectory. Without it, I see the directory
where the terminal was opened, which is correct on the first prompt
and stale after a `cd`. Both are clearly correct, just at different
granularities.

---

### REQ-TPANEL-005: Output Keeps Coming When I Look Away

WHEN I collapse the terminal panel to focus on something else
THE SYSTEM SHALL keep my shell running and continue receiving output
(no scrollback loss)

WHEN output arrives while the panel is collapsed
THE SYSTEM SHALL show me an unread badge on the panel header so I can
tell something happened

WHEN I expand the panel back open
THE SYSTEM SHALL show me the latest output, scrolled where I'd
expect, with the unread badge cleared

**Rationale:** A long `cargo build` should keep building when I look
at the diff viewer or compose a message. Collapsing the panel should
mean "I'm not watching" — not "kill the work I started." The unread
badge is the cheap hint that "something happened while you were
away" so I can decide whether to peek.

---

### REQ-TPANEL-006: Be Told When Shell Integration Is Missing, and Get Help Enabling It

WHEN the system cannot detect shell integration markers from my shell
within a few seconds of opening the terminal
THE SYSTEM SHALL surface this state in the panel header (a hover hint
naming my shell and indicating "shell integration not detected")

WHEN I click into that hint
THE SYSTEM SHALL show me the snippet for my shell, the rc file path
to add it to, and a copy-to-clipboard action

WHEN I want Phoenix to do it for me
THE SYSTEM SHALL offer a "Let Phoenix set this up for me" option that
spawns a guided conversation in my home directory, pre-loaded with a
prompt that inspects my dotfiles and applies the snippet on my behalf
— I review and Send before any file changes

WHEN the guided conversation fails to spawn (e.g. my home directory
is unavailable, or a backend error)
THE SYSTEM SHALL surface the failure visibly to me, not only via the
developer console

WHEN my shell is not in the snippet catalog
THE SYSTEM SHALL still tell me about the OSC 133 / OSC 7 contract so
I can wire it up myself

**Rationale:** Without shell integration, the rich HUD (REQ-TPANEL-003)
degrades to a coarser indicator. The setup CTA is the bridge: tell me
why the HUD is degraded, give me the path to fix it, automate that
path if I prefer. The "spawn a guided conversation" option keeps me
in control — I see the prompt, review what Phoenix proposes, and Send
when I'm ready. No silent dotfile edits.

---

### REQ-TPANEL-007: The Terminal Looks Right in Light or Dark Mode

WHEN I'm using the app in dark or light mode
THE SYSTEM SHALL render the terminal with colors that match the rest
of the app

WHEN I toggle modes
THE SYSTEM SHALL update the terminal colors immediately, without
flashing the old theme or interrupting my session

**Rationale:** A mismatched terminal theme draws the eye and breaks
the visual coherence of the app. Toggling should be instantaneous —
no PTY teardown, no scrollback loss, just the colors switching live.

---

### REQ-TPANEL-008: Don't Silently Take Over a Terminal I Have Open Elsewhere

WHEN I have a conversation's terminal open in one tab and try to
open it in another
THE SYSTEM SHALL detect the conflict (the backend rejects the
duplicate connection per REQ-TERM-001 / REQ-TERM-003)
AND SHALL distinguish this case from a generic connection failure

WHEN the conflict is detected
THE SYSTEM SHALL offer me an explicit "Reclaim this terminal" action
— clicking it disconnects the other tab and attaches this one

THE SYSTEM SHALL NOT auto-reclaim. Silently kicking my other tab
without consent is unacceptable.

THE SYSTEM SHALL NOT fold the conflict into a generic "Connection
error" with no path forward. That leaves me stuck with no idea what
happened.

**Rationale:** The single-attach constraint is correct on the backend
side — two tabs sharing a PTY's input would be a mess. But the user
experience of the rejection has to be specific: tell me what
happened, give me a way to reclaim if I really want to. Currently NOT
implemented (the close path treats every failure as generic
"Connection error"); this requirement is the spec target. See the
executive's status table.
