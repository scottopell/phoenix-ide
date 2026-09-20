# Durable inline SVG presentation

## User story

As a Phoenix user, I need an agent's generated charts and illustrations to appear inline and remain available with the conversation, so I can understand, inspect, and download results without locating server scratch files.

## Requirements

### REQ-SVG-001: File-based publication

WHEN an agent invokes `present_svg` in Direct or Work with filesystem authority
THE SYSTEM SHALL accept a resolved absolute server-local filename, a nonempty plain-text title of at most 200 Unicode scalar values, and a nonempty plain-text description of at most 2000 Unicode scalar values
AND SHALL reject control characters in either text field
AND SHALL read only a bounded regular file using the host process's applicable filesystem permissions
AND SHALL NOT expand shell expressions or remove the source.

THE SYSTEM SHALL NOT advertise publication in Explore, filesystem-free coordinator contexts, or Explore subagents.

### REQ-SVG-002: Static SVG policy

THE SYSTEM SHALL parse XML with DTD/entity resolution and resource fetching disabled
AND SHALL accept only its documented static SVG element, attribute, and styling subset
AND SHALL reject scripts, event handlers, embedded HTML, navigation, animation, embedded image payloads, and external references in attributes or CSS
AND SHALL preserve accepted visual content without silently stripping unsupported features.

THE SYSTEM SHALL accept geometry and presentation attributes only on compatible elements, local references only to compatible resource types, and stylesheets only with the documented simple-selector grammar
AND SHALL apply the same presentation-property restrictions to inline declarations and require each stylesheet selector with matches to have a compatible target for each declaration, while permitting broad selectors and inherited container styling
AND SHALL enforce documented parent-child content models so visual elements and text cannot be placed where the SVG renderer ignores them
AND SHALL require a validation-backed value at the storage publication boundary so unvalidated bytes and independently supplied dimensions cannot be published.

THE SYSTEM SHALL support bounded local references for glyph reuse, clipping, and gradients
AND SHALL reject cycles, excessive expansion, invalid/non-finite geometry, extreme dimensions, and byte/structure/path/reference limits.

### REQ-SVG-003: Durable ownership and replay

WHEN publication succeeds
THE SYSTEM SHALL have atomically committed immutable accepted bytes, presentation metadata, trusted conversation ownership, and trusted tool invocation identity
AND SHALL return only a compact reference and static-validation outcome, without claiming visual inspection.

WHEN the same invocation is replayed
THE SYSTEM SHALL return its first committed snapshot without depending on staging-file availability.

WHEN restart recovery materializes an interrupted publication whose exact invocation has a committed snapshot
THE SYSTEM SHALL recover its compact successful reference into tool-result history
AND SHALL retain an interrupted error when that invocation has no committed snapshot.

Invocation identity SHALL include the owning assistant-message identity and provider tool-use ID within the conversation
AND SHALL distinguish separate assistant messages even when their provider tool-use IDs repeat.

THE SYSTEM SHALL preserve snapshots across source replacement/deletion, process restart, reconnect, worktree removal, and workscope retirement while the publishing conversation is retained
AND SHALL delete snapshots when that conversation is deleted.

WHEN cancellation or failure occurs before commit
THE SYSTEM SHALL NOT report publication success or leave an unowned snapshot.

WHEN commit finishes but cancellation prevents delivery of the tool result
THE SYSTEM SHALL retain the conversation-owned snapshot recoverable by invocation identity.

### REQ-SVG-004: Safe authenticated retrieval

WHEN a client requests preview, source, or download
THE SYSTEM SHALL apply the instance's existing authentication policy and match both opaque artifact ID and owning conversation ID
AND SHALL NOT resolve an arbitrary filesystem path from that request.

WHEN a client requests an artifact through a read-only share
THE SYSTEM SHALL validate the share token on every preview, source, and download request
AND SHALL derive the owning conversation only from that token
AND SHALL reject invalid or revoked tokens and artifacts outside that conversation with HTTP 404
AND SHALL allow these reads without the instance password, using the same accepted snapshots and serving protections as owner access.

THE SYSTEM SHALL serve only accepted bytes with explicit MIME, nosniff, private caching, same-origin resource policy, and restrictive sandbox CSP
AND SHALL prevent direct SVG navigation from becoming an active same-origin document
AND SHALL serve source as text and downloads as attachments.

### REQ-SVG-005: Inline accessible presentation

WHEN a successful publishing result appears in a conversation
THE SYSTEM SHALL display one visible artifact card at its chronological tool location, including compact presentation
AND SHALL keep ordinary tool activity inspectable without duplicating the visual.

THE SYSTEM SHALL render preview and expansion only in image context
AND SHALL provide title, accessible description, bounded responsive preview, aspect-ratio preservation, loading/error fallback, expand/zoom, escaped source view, and download controls
AND SHALL support narrow layouts, light/dark themes, keyboard operation, Escape dismissal, and focus restoration.

### REQ-SVG-006: Existing history and client compatibility

THE SYSTEM SHALL persist compact references through ordinary tool-result history and replay
AND SHALL keep older messages and clients without SVG-specific rendering readable through title, description, and reference text
AND SHALL preserve Mermaid, ordinary SVG code examples, and generic tool inspection behavior.

### REQ-SVG-007: Actionable guidance and failures

THE SYSTEM SHALL distinguish invalid input, policy rejection, limits, read failure, and persistence failure using bounded actionable errors without logging file contents.

THE SYSTEM SHALL teach agents to generate SVG with code or chart libraries, resolve platform-temp filenames before invocation, keep staging until success, and distinguish validation from visual inspection
AND SHALL document supported features and concrete limits.
