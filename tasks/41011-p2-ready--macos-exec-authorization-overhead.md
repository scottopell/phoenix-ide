On this macOS host (Santa in Monitor mode + SentinelOne), process launches go through cold phases that cost ~270ms each one-at-a-time and up to ~800ms under 4-way parallelism, versus ~5-12ms when warm. nextest runs every test in its own process (and double-spawns), so a cold phase turns the ~4,400-test suite into 20+ minutes and blows the 900s check budget. Tests that launch git/sh/tmux pay the same cost.

Done so far: PHOENIX_CHECK_CARGO_TEST_TIMEOUT_SECS override (set to 3600 in this host .phoenix-ide.env), and a standalone repro script for IT outside the repo (~/dev/exec-overhead-repro).

Open: IT investigation of the cold phases; once understood, decide whether check should prefer in-process cargo test for pure crates and whether to drop the host override.
