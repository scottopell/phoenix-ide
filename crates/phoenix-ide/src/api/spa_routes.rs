//! Single source of truth for the client-side (SPA) routes.
//!
//! The UI is a single-page app: a set of client routes that all render from one
//! `index.html` shell. Two server-side concerns must agree on exactly that set,
//! and historically drifted apart:
//!
//!   1. The router (`handlers::create_router`) must serve the SPA shell for each
//!      route, or a direct load / refresh / bookmark of it 404s — only in-app
//!      navigation works.
//!   2. The auth middleware (`auth::is_exempt_path`) must treat each route as
//!      public, or — when a password is configured — a direct load 401s before
//!      React ever mounts, so the in-app login screen can't render. Serving the
//!      shell is safe: it is the same public bundle for every user and carries
//!      no secrets; all privileged data is fetched later via auth-gated `/api`.
//!
//! A route present in one list but not the other is a latent 404 or 401. Both
//! consumers derive from `SPA_ROUTES` here, so that drift is unrepresentable:
//! there is no second list to forget. Adding a React `<Route>` means adding one
//! entry here, and the router and the auth exemption both pick it up.

/// A client-side route served by the SPA shell.
#[derive(Clone, Copy)]
pub enum SpaRoute {
    /// A fixed path with no parameters (e.g. `/new`). Matched by equality.
    Exact(&'static str),
    /// A parameterised route. `pattern` is the axum path pattern used for
    /// registration (e.g. `/c/:slug`); `prefix` is the literal leading segment
    /// used for runtime matching against a concrete request path (e.g. `/c/`).
    Param {
        prefix: &'static str,
        pattern: &'static str,
    },
}

impl SpaRoute {
    /// The axum route pattern to register with `Router::route`.
    pub fn pattern(self) -> &'static str {
        match self {
            SpaRoute::Exact(p) => p,
            SpaRoute::Param { pattern, .. } => pattern,
        }
    }

    /// Whether a concrete request path addresses this SPA route.
    pub fn matches(self, path: &str) -> bool {
        match self {
            SpaRoute::Exact(p) => path == p,
            SpaRoute::Param { prefix, .. } => path.starts_with(prefix),
        }
    }
}

/// Every client-side route, in one place. The router serves each as the SPA
/// shell and the auth middleware exempts each; add a route here once and both
/// stay in sync. Keep this aligned with the React `<Route>` table in
/// `ui/src/App.tsx`.
pub const SPA_ROUTES: &[SpaRoute] = &[
    SpaRoute::Exact("/"),
    SpaRoute::Exact("/new"),
    SpaRoute::Exact("/terminal"),
    SpaRoute::Exact("/about"),
    SpaRoute::Exact("/usage"),
    SpaRoute::Exact("/global"),
    SpaRoute::Param {
        prefix: "/global/",
        pattern: "/global/:slug",
    },
    SpaRoute::Exact("/settings/llm-language"),
    SpaRoute::Exact("/codex/login"),
    SpaRoute::Param {
        prefix: "/c/",
        pattern: "/c/:slug",
    },
    SpaRoute::Param {
        prefix: "/chains/",
        pattern: "/chains/:rootConvId",
    },
];

/// True when `path` is one of the SPA client routes.
pub fn is_spa_route(path: &str) -> bool {
    SPA_ROUTES.iter().any(|r| r.matches(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_client_routes_match() {
        assert!(is_spa_route("/"));
        assert!(is_spa_route("/new"));
        assert!(is_spa_route("/terminal"));
        assert!(is_spa_route("/about"));
        assert!(is_spa_route("/usage"));
        assert!(is_spa_route("/global"));
        assert!(is_spa_route("/global/coordinator-id"));
        assert!(is_spa_route("/settings/llm-language"));
        assert!(is_spa_route("/codex/login"));
        assert!(is_spa_route("/c/some-slug"));
        assert!(is_spa_route("/chains/root-conv-id"));
    }

    #[test]
    fn server_namespaces_do_not_match() {
        // Privileged / server-owned paths must NOT be treated as SPA routes,
        // otherwise the auth exemption that consumes this would leak them.
        assert!(!is_spa_route("/api/conversations"));
        assert!(!is_spa_route("/api/models"));
        assert!(!is_spa_route("/preview/some/file.html"));
        assert!(!is_spa_route("/assets/index-abc.js"));
        assert!(!is_spa_route("/share/c/some-slug"));
    }

    #[test]
    fn exact_routes_do_not_prefix_match() {
        // `/newish` must not be mistaken for `/new`.
        assert!(!is_spa_route("/newish"));
        assert!(!is_spa_route("/terminals"));
    }

    #[test]
    fn param_routes_match_by_prefix() {
        assert_eq!(
            SpaRoute::Param {
                prefix: "/c/",
                pattern: "/c/:slug",
            }
            .pattern(),
            "/c/:slug"
        );
    }
}
