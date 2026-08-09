# Diagnose CDP modifier-chord pathology

Systematically isolate the browser-key-press performance and post-command responsiveness anomaly across letters, modifiers, focus targets, listener phases, and CDP event sequences. Preserve trusted CDP behavior, retain raw evidence, convert proven behavior into the smallest durable regression or implementation fix, and restore all temporary diagnostics.


## Outcome

The anomaly was not specific to Ctrl+K. Trusted printable CDP events for Ctrl+A/F/K/L/P/X and Shift/Alt/Meta+K all showed multi-second follow-up delays when the page did not cancel default behavior. Event capture revealed phantom trusted keys such as `Unidentified`/`NumpadDivide` and `Enter`/`NumpadEnter` alongside the requested letter.

Field isolation proved that Phoenix supplied Windows virtual-key values as both `windowsVirtualKeyCode` and host-specific `nativeVirtualKeyCode`. On macOS, Chrome interpreted the latter using the native hardware keycode table. Omitting only `nativeVirtualKeyCode` removed phantom events and delays for every tested combination while preserving trusted events, modifiers, capture delivery, plain input insertion, shortcut non-insertion, focus, and URL.

The durable regression verifies Ctrl+K's keydown/keyup lifecycle, key identity, modifier, trusted status, lack of phantom native keys, lack of input insertion, and immediate post-chord evaluation. Restoring the original field deliberately makes the regression fail. Five exact post-fix samples reduced median Ctrl+K targeted CPU from 15.11s to 1.29s and isolated elapsed from 9.72s to 0.56s. The key family passed 4/4, broad browser tests passed 34/34, and `./dev.py check --all` passed all 20 checks.
