---
created: 2026-04-19
priority: p3
status: ready
artifact: pending
---

# voice-persistent-listening-mode

## Summary

Add a persistent-listening mode to `VoiceRecorder`. Today the mic is
one-shot: arm it, speak one utterance, it stops, you send, and you
have to click the mic again for the next turn. Add an opt-in mode
where, once armed, the mic auto-resumes listening after each sent
message — so the user can hold a continuous back-and-forth with the
agent by voice without reaching for the mouse.

## Context

`VoiceRecorder` is already wired into both the new-conversation page
(`ui/src/pages/NewConversationPage.tsx:139,204`) and the in-conversation
composer (`ui/src/components/InputArea.tsx:698`). The current behavior
is single-utterance: `VoiceRecorder` stops on the recognizer's `end`
event, on Escape, or on outside click. After the message is sent, the
user has to click the mic again.

For quick idea-capture and iterative planning (especially in Explore
with a fast model — see task 24687), the single-shot model is the
biggest friction point. A "persistent" toggle that auto-resumes
listening whenever the conversation phase returns to `idle` would let
the user think out loud, push back, refine, and just keep talking.

## Scope

Two distinct modes on the voice input:

1. **One-shot (current behavior, default).** Click mic → listen for
   one utterance → stop → user sends → mic stays off until next click.
2. **Persistent (new, opt-in).** Click the persistent toggle → mic
   listens → on user send, mic briefly goes idle, then auto-resumes
   listening when the conversation phase returns to `idle`. Repeats
   until user explicitly disarms.

### UI

- Add a persistent-mode toggle near the existing mic button — small
  pin/lock affordance ("keep listening") adjacent to `VoiceButton`.
- Visible listening indicator is the existing one; extend it with a
  subtle "persistent" marker when in that mode so it's obvious the
  mic will come back on its own.
- Disarm paths: click the persistent toggle off, click the mic button,
  press Escape, or click outside. Any of these fully stop listening.

### Behavior

- Persistent mode is per-conversation (or maybe per-session — see
  open questions).
- On user send, `VoiceRecorder` stops the current recognizer (required
  so the recognizer doesn't ingest the next LLM turn or silence
  timeouts).
- When the conversation phase atom (`ui/src/conversation/atom.ts`)
  transitions back to `idle` AND persistent mode is still on, restart
  the recognizer after a short debounce (~300-500ms) so the user's
  post-send breath / reaction doesn't get captured.
- If the phase goes to `error` or `terminal`, persistent mode
  disarms itself and surfaces a small toast or inline notice.

## Out of scope

- Changing anything about the model or backend. Persistent listening
  is purely UI. (Explore defaulting to Haiku is tracked in 24687.)
- Hotkey / global-shortcut activation for voice. Nice later, not MVP.
- Mobile-specific tuning.
- Any transcription-quality changes (still Web Speech API).

## Acceptance criteria

- [ ] One-shot mode behaves exactly as it does today (regression
      check — no surprise changes).
- [ ] A persistent-mode toggle exists on both the new-conversation
      input and the in-conversation composer.
- [ ] When persistent is on and the user sends a message, the mic
      stops while the agent runs, then auto-resumes listening once the
      conversation phase returns to `idle`.
- [ ] Debounce on auto-resume is short enough to feel responsive but
      long enough not to capture the user's own reaction noise
      (tune empirically, start at 300ms).
- [ ] Persistent mode is visibly distinct from one-shot (additional
      indicator or different icon state).
- [ ] Clear disarm paths: toggle off, click mic, Escape, outside
      click — any of these fully stop listening.
- [ ] Error or terminal phase auto-disarms and informs the user.

## Open questions

- Should the persistent toggle state persist across reloads (localStorage
  per conversation), or always reset to off? Default: reset to off,
  since re-arming requires a user gesture anyway (browser autoplay).
- Exact debounce duration — start at 300ms and iterate.
- Should there be a hard timeout (e.g. auto-disarm after N minutes of
  silence) to avoid a forgotten open mic? Probably yes; pick a
  conservative default like 5 minutes and surface it.

## Notes

- Primary site: `ui/src/components/VoiceInput/VoiceRecorder.tsx`
  (add persistent-mode prop or a new wrapping component).
- Composer wiring: `ui/src/components/InputArea.tsx:698` and
  `ui/src/pages/NewConversationPage.tsx:139,204`.
- Phase atom to subscribe to: `ui/src/conversation/atom.ts:40-58`
  (`phase: 'idle' | 'awaiting_llm' | ...`).
- Existing voice spec: `specs/voice-input/` — update once this lands.
- Related task: 24687 (Explore defaults to Haiku) — orthogonal but
  ships together experientially, since the persistent loop only feels
  right when each turn is fast.
