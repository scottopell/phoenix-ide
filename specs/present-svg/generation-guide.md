# Generate and publish static SVG

`present_svg` is available in Direct/Work and Work subagents. It reads a file on the Phoenix server, using the host process's read permissions. It does not expand variables or `~`. Explore is excluded: its command scratch and temporary-directory fallback can disappear when a command finishes.

Generate with code or a chart library; do not transcribe a large SVG into tool arguments. Create a platform-temp staging directory:

```sh
artifact_dir=$(mktemp -d "${TMPDIR:-/tmp}/phoenix-svg.XXXXXX")
# Generate "$artifact_dir/disk-usage.svg" with code or a chart library.
printf '%s\n' "$artifact_dir/disk-usage.svg"
```

Pass the printed, resolved absolute filename in a subsequent call:

```text
present_svg(path="<printed absolute filename>",
            title="Largest storage consumers",
            description="Horizontal bars compare measured directory sizes in GiB; free space is shown separately.")
```

Clean up staging only after publication succeeds. The tool preserves the input file and stores its own immutable snapshot. A new invocation produces a new card; replay of the same invocation returns the first snapshot. Success means static-policy validation and persistence, not visual inspection. If layout quality matters, inspect a rendered image before claiming the chart looks correct.

For disk usage, put exact measured GiB values beside horizontal bars. Report free space separately from directory usage. Do not sum nested breakdowns into parent totals: a parent's measured size already includes its children. Avoid implying the fixture numbers are a measurement of the user's machine.

## Static subset

The root must be SVG in the SVG namespace. Shapes, paths, text/tspan, groups, definitions, local use references (including glyph paths), clipping and linear/radial gradients are supported. Styling is a restricted set of presentation properties in attributes, inline declarations, and simple stylesheets. Export with self-contained geometry, no fonts or images fetched from elsewhere.

Scripts, event handlers, links, embedded HTML, animation, images/data payloads, external URLs, DTD/entities, processing instructions, foreign metadata, nested SVG viewports, filters and patterns are rejected. Unsupported features produce an actionable error rather than a modified chart. Attribute entity encoding and namespace prefixes do not bypass policy. CSS escapes, comments, at-rules and complex selectors are outside the supported styling subset.

Matplotlib commonly emits a DOCTYPE and RDF metadata by default. Export without them, or explicitly remove just those nonvisual nodes before publication; do not remove unsupported visual content to force a pass. The representative generator and fixture live under `crates/phoenix-tools/src/present_svg/fixtures/`. They preserve glyph paths, styling, clipping and local references.

## Limits

| Resource | Maximum |
| --- | --- |
| Source bytes | 2 MiB |
| Title / description | 200 / 2000 Unicode scalar values, nonempty, no control characters |
| Filename | 4096 bytes, absolute, readable regular file; final symlinks rejected |
| Elements / XML nodes | 20,000 / 50,000 |
| Element nesting | 64 |
| Attributes / path segments | 100,000 each |
| Local references | 10,000; no reference cycles |
| Expanded rendering complexity | 500,000 weighted nodes/segments/text bytes |
| Viewport width or height | 16,384 pixels |
| Viewport area | 64 million pixels |
| Numeric magnitude | 10 million |
| Stylesheet rules / selectors / declarations | 256 / 256 / 1024 |

Use explicit dimensions in pixels or common absolute units (pt/in/cm/mm/pc), or a valid viewBox. Simplify charts that exceed the bounds. Errors distinguish invalid input, policy rejection, limits, read failures, and persistence failures; retain staging while correcting or retrying publication.

Supported style properties are `fill`, `stroke`, `stop-color`, `color`, `clip-path`, `opacity`, `fill-opacity`, `stroke-opacity`, `stop-opacity`, `stroke-width`, `stroke-dashoffset`, `stroke-miterlimit`, `stroke-dasharray`, `stroke-linecap`, `stroke-linejoin`, `fill-rule`, `clip-rule`, `font-size`, `letter-spacing`, `word-spacing`, `font-family`, `font-style`, `font-weight`, `text-anchor`, `dominant-baseline`, `alignment-baseline`, `display`, `visibility`, `overflow`, `vector-effect`, `shape-rendering`, and `text-rendering`. Values are restricted static keywords, bounded numbers/lengths, colors, and local font names. Stylesheet selectors are comma-separated simple `*`, element, `.class`, or `#id` selectors; no combinators or pseudo-selectors. Local `url(#id)` is supported in fill/stroke/clip-path attributes and inline style, not stylesheet rules.

Only XML 1.0 with absent or UTF-8 encoding declarations is accepted. Minimum viewport/viewBox dimensions are 0.000001. The numeric limit applies to individual numbers and composed transform coefficients; transformed positions can be larger. Relative em/ex lengths and percentage font sizes are unsupported to avoid inherited exponential scaling. Each element has at most 64 attributes and each attribute at most 512,000 bytes. Each transform has at most 64 operations; dash arrays have at most 256 numbers.
