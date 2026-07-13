# Virtual Transcript fixtures

`v1/scenarios.json` is the portable conformance corpus for Phoenix Virtual Transcript implementations. Web, iOS, and other clients should consume the same JSON and validate it against `v1/schema.json` (JSON Schema draft 2020-12) before running platform-specific conformance checks.

## Units

All extents and offsets in the corpus are expressed in CSS pixels (`metadata.unit = "css_px"`). iOS should treat the numeric values as points for conformance because UIKit points and CSS pixels both represent device-independent layout units. Do not multiply by device scale; a 180 px viewport in the corpus maps to a 180 pt viewport in an iOS fixture.

## Shared semantics

The scenarios describe semantic transcript behavior rather than DOM details: stable row keys, canonical and alias message IDs, measured extents, viewport offsets, visible ranges, reading anchors, follow-tail state, and deterministic expectations. Platform renderers may differ visually, but they should preserve these values and calculations when restoring anchors, handling prefix insertion, reacting to resize, resolving aliases, appending streamed rows, and superseding restore commands.

`visibleRange` is the inclusive index range of rows with positive-area intersection between the half-open row interval `[rowStart, rowEnd)` and the half-open viewport interval `[offset, offset + extent)`. A row that only touches a viewport boundary is not visible. This is intentionally distinct from an implementation's overscan/rendered range, which may include additional rows outside the viewport for smooth scrolling.

The schema requires exactly one scenario for each of the seven stable IDs. Runtime adapters should independently reject missing or duplicate IDs instead of relying only on schema validation. Scenario identity is defined by `id`, not array position; conforming adapters accept any scenario order.
