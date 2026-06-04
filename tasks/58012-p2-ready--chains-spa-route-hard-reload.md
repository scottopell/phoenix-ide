Client-side SPA routes must be explicitly registered with serve_spa in the backend router (crates/phoenix-ide/src/api/handlers.rs) AND added to the auth-exempt list (crates/phoenix-ide/src/api/auth.rs is_exempt_path), or a hard-load/refresh of that URL 404s in production (where the embedded UI is served by the binary rather than Vite).

The chain page route /chains/:rootConvId is declared as a client route in ui/src/App.tsx but is NOT registered with serve_spa and is NOT auth-exempt. Result: navigating to a chain via client-side routing works, but refreshing or deep-linking /chains/<rootConvId> returns the "404 - UI not found" page in prod.

Fix: register .route("/chains/:rootConvId", get(serve_spa)) alongside the other SPA routes, and add a path.starts_with("/chains/") arm to is_exempt_path (plus an assertion in the exempt_paths_are_correct test). Verify in a prod build that a hard-load of a chain URL serves the SPA shell.
