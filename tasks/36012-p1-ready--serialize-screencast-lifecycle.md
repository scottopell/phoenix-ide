# Make screencast stop and restart one owned lifecycle

The last viewer currently drops `ScreencastBroker`, whose `Drop` asynchronously spawns `Page.stopScreencast`. A concurrent new attach can issue `Page.startScreencast` before that stop arrives, allowing the stale stop to cancel the replacement stream. The former browser integration test hid this with a 100 ms sleep.

Move start/stop ownership into a session-scoped lifecycle state that makes idle, running, and stopping structurally distinct. A new attach must await the exact prior stop before starting, while concurrent viewers still share one broker. Add deterministic overlap tests with explicit stop/start acknowledgements; do not use sleeps.
