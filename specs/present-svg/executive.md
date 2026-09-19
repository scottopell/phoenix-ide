# Durable inline SVG presentation — Executive Summary

## Requirements Summary

Agents publish code-generated static SVG files as durable inline charts. Users can expand, inspect source, and download without accessing server paths. Explore publication is excluded because its staging lifetime is not reliably cross-call.

## Technical Summary

Strict XML/SVG validation precedes an atomic conversation-owned database snapshot. Existing tool-result text carries a compact typed reference; web UI reads it into an image-only card. Authenticated routes match conversation and artifact IDs. See ADR-058.

## Status Summary

| Requirement | Status | Evidence |
| --- | --- | --- |
| REQ-SVG-001 | In progress | Tool and mode registration |
| REQ-SVG-002 | In progress | Validator and hostile fixtures |
| REQ-SVG-003 | In progress | Database transaction, replay and retention tests |
| REQ-SVG-004 | In progress | Owned API routes and auth tests |
| REQ-SVG-005 | In progress | Card component, UI tests and browser QA |
| REQ-SVG-006 | In progress | Ordinary tool-result carrier |
| REQ-SVG-007 | In progress | Tool description and generation guide |

## Verification

Qualification is underway. Evidence will distinguish automated coverage from actual browser and runtime journeys.
