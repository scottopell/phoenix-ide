---
created: 2026-02-04
priority: p3
status: pending
---

# Add ETag Support to Backend API

## Summary

Implement ETag headers in the backend API to enable conditional requests and reduce bandwidth.

## Context

During performance testing, we discovered the backend doesn't send cache headers (ETag, Last-Modified). Adding these would enable conditional requests (If-None-Match) and 304 responses.

## Acceptance Criteria

- [ ] Add ETag generation for conversation endpoints
- [ ] Support If-None-Match header
- [ ] Return 304 Not Modified when appropriate
- [ ] Add Cache-Control headers
- [ ] Update enhancedApi to use ETags
- [ ] Track bandwidth savings

## Notes

- Use content hash or timestamp for ETag
- Consider different cache policies for different endpoints
- List endpoints: short TTL (1-5 minutes)
- Static data: long TTL (1 hour+)
