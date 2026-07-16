# Resolve e2e idle/SSE terminal barrier timeout

The `text_streaming` e2e previously timed out after observing `idle` because its SSE completion barrier was not bound to the exact user turn. Main now carries the exact-turn barrier fix, and the full repository e2e lane passes with it. No timeout increase or polling workaround was used.
