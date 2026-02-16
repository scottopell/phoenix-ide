---
created: 2026-02-04
priority: p3
status: pending
---

# Add Performance Regression Tests

## Summary

Create automated tests to prevent performance regressions in navigation and caching.

## Context

We fixed navigation performance issues but have no automated tests to prevent regressions. Need to add performance benchmarks.

## Acceptance Criteria

- [ ] Test cached navigation is <100ms
- [ ] Test cache hit rate stays >80%
- [ ] Test IndexedDB operations performance
- [ ] Test memory usage doesn't grow unbounded
- [ ] Add to CI pipeline
- [ ] Alert on performance regression

## Notes

- Use Playwright for E2E performance tests
- Measure using performance.mark/measure
- Test with realistic data (100+ conversations)
- Consider using Lighthouse CI
