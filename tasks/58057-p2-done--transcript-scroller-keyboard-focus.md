The conversation transcript cannot be scrolled by keyboard unless focus already
sits inside it.

On the chat route `.desktop-main:has(.chat-main-area)` and `#main-area.chat-main-area`
are both `overflow: hidden`, and the only scrollable box is `#messages`
(`.virtual-transcript`), which carries no tabindex. Browsers scroll the nearest
scrollable *ancestor* of the focused element; with focus on the body, every
ancestor is locked and the scroller is a descendant, so PageDown/Home/End/arrows
move nothing at all.

This is an accessibility gap on the primary reading surface of the app,
independent of any scroll-policy behaviour.

It also forces the scroll policy to guess. PR #700 had to add a containment check
to the key handler because body-targeted keys were being treated as viewport
movement — cancelling an active positioning command and taking ownership for a
scroll that could not physically happen. That guard is correct but it only stops
the handler lying; it does not make the keyboard work. With the scroller
focusable, "the key reached the transcript" and "the key will scroll the
transcript" become the same statement, and the guard becomes a tautology rather
than an approximation.

The codebase has already met this need once and solved it locally: MessageList's
`restore-focus` command stamps `tabindex="-1"` on the scroller imperatively at
runtime and focuses it. That is the signature of a missing element property being
patched at a call site.

Proposed: give `.virtual-transcript` a declarative `tabIndex={-1}` (or `0`, if it
should be in the tab order) and focus it on chat-route entry, then remove the
imperative stamping in the find restore-focus path.

Open questions for whoever picks this up:
- `-1` (programmatically focusable, out of tab order) vs `0` (tabbable). `0` adds
  a tab stop before the composer; `-1` requires an explicit focus on route entry.
- Whether focusing on route entry steals focus from the message composer, which
  is the more common thing to want focused.
- Whether a visible focus ring on the scroller is wanted or should be suppressed.

Raised while reviewing PR #700's container structure.

Resolved:
- `tabIndex={0}`, declared on `.virtual-transcript` in VirtualTranscript. `-1`
  would only permit programmatic focus, leaving a keyboard reader with no way to
  reach the port — which is the gap.
- No focus on route entry. The tab stop is enough, and auto-focus would take the
  composer's focus, which is the more common thing to want focused.
- Visible ring, `outline-offset: -2px` so the clipping ancestors cannot hide it.
- `role="region"` + `ariaLabel` so the new stop is not anonymous.
- The imperative `tabindex="-1"` stamp in the find restore-focus path is removed;
  `focusOrigin` is always a former `document.activeElement`, so the stamp only
  ever mattered for the scroller fallback.
- REQ-VT-013 records the requirement.
