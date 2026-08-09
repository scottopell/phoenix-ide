# Cap each development check at four cores

Keep each `./dev.py check` invocation conceptually bounded to four CPU cores without shared host coordination. Explicitly cap Rust and Vitest workers, prevent heavyweight lanes from multiplying beyond the per-check budget, expose the selected plan, and validate process/load behavior including macOS fseventsd pressure under concurrent development.
