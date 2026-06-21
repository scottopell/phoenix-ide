SPA client routes are enumerated in TWO places that must stay in lockstep, and drift between them is a recurring bug:

1. `create_router` in crates/phoenix-ide/src/api/handlers.rs — each client route needs an explicit `.route(path, get(serve_spa))` or a direct load/refresh/bookmark 404s in production.
2. `is_exempt_path` in crates/phoenix-ide/src/api/auth.rs — each client route must be auth-exempt or, when PHOENIX_PASSWORD is set, a direct load 401s before the SPA shell (and its login screen) can render.

Two real bugs from this drift have already been fixed surgically (/terminal was missing from is_exempt_path; /chains was missing from both). The structural fix is a single source of truth for "is this a client-side SPA route" feeding both the router and the auth exemption, so adding a new page cannot silently 404 or 401.

Constraint: the router matches axum patterns (/c/:slug, /chains/:rootConvId) while is_exempt_path does runtime prefix/exact matching, so the unification needs a shared predicate (e.g. is_spa_route(path)) used by auth, plus either a router fallback scoped to non-/api/non-asset GETs or a generated route list — without changing the 404 semantics for unknown /api and asset paths.
