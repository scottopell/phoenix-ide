---
created: 2026-02-04
priority: p3
status: pending
---

# Optimize Bundle Size

## Summary

The current bundle is 226KB (71KB gzipped). Investigate opportunities to reduce bundle size.

## Context

During Phase 2c, we noted the bundle size increased by 3KB. While reasonable, there may be opportunities to reduce the overall size.

## Acceptance Criteria

- [ ] Analyze bundle with webpack-bundle-analyzer
- [ ] Identify large dependencies
- [ ] Consider code splitting for routes
- [ ] Lazy load heavy components (markdown, syntax highlighting)
- [ ] Remove unused dependencies
- [ ] Target: <200KB uncompressed

## Notes

- Current: 226KB (71KB gzipped)
- Consider dynamic imports for modals
- Tree-shake unused utility functions
- Minimize IndexedDB wrapper code
