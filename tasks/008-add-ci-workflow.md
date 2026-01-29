---
created: 2026-01-29
priority: p2
status: ready
---

# Add CI Workflow for Automated Testing

## Summary

Set up GitHub Actions (or similar) to run tests automatically on push/PR.

## Context

The project now has 67 tests including property-based tests. These should run automatically to catch regressions.

## Acceptance Criteria

- [ ] Create `.github/workflows/ci.yml`
- [ ] Run `cargo test` on push to main
- [ ] Run `cargo test` on pull requests
- [ ] Run `cargo clippy` for lint checks
- [ ] Run `cargo fmt --check` for formatting
- [ ] Cache cargo dependencies for faster builds

## Notes

Basic workflow:

```yaml
name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test
      - run: cargo clippy -- -D warnings
      - run: cargo fmt --check
```

Consider adding:
- Release builds
- Coverage reporting
- Separate job for slow property tests
