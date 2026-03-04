---
created: 2026-01-29
priority: p3
status: blocked
blocked_by: 019-mvp-ui
---

# Implement static asset serving (REQ-API-010)

## Summary

The API server does not serve static frontend assets at `/`. Currently returns 404.

## Context

Discovered during QA validation (REQ-API-010). The spEARS spec requires serving the frontend from the main server. Currently the UI runs as a separate dev server.

This may be intentional for dev workflow, but production deployment needs a solution.

## Acceptance Criteria

- [ ] Decide: embed frontend in binary or serve from filesystem
- [ ] Implement static file serving at `/` and `/assets/*`
- [ ] Fallback to index.html for SPA routing
- [ ] Document deployment configuration

## Notes

Options:
1. `rust-embed` to compile assets into binary
2. `tower-http::ServeDir` for filesystem serving
3. Reverse proxy in production (nginx/caddy)
