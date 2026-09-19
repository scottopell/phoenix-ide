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

Matplotlib commonly emits a DOCTYPE and RDF metadata by default. Export without them, or explicitly remove just those nonvisual nodes before publication; do not remove unsupported visual content to force a pass. The representative generator and fixture live under `crates/phoenix-svg/src/fixtures/`. They preserve glyph paths, styling, clipping and local references.

## Reference and geometry rules

References must resolve to a compatible element: fill/stroke paints and gradient `href` target linear or radial gradients; `clip-path` targets `clipPath`; `use` targets a group, shape, path, text, or another `use`. A matching ID alone is insufficient. Both gradient types may inherit shared gradient properties and stops from either gradient type.

Inside `clipPath`, `use` must directly reference a shape/path or text, rather than a group or another `use`.

Geometry attributes are element-specific:

| Attributes | Supported elements |
| --- | --- |
| `x`, `y` | `rect`, `text`, `tspan`, `use` |
| `dx`, `dy` | `text`, `tspan` |
| `x1`, `y1`, `x2`, `y2` | `line`, `linearGradient` |
| `cx`, `cy` | `circle`, `ellipse`, `radialGradient` |
| `fx`, `fy`, `fr` | `radialGradient` |
| `width`, `height` | Root `svg`, `rect` |
| `rx`, `ry` | `rect`, `ellipse` |
| `r` | `circle`, `radialGradient` |
| `d` | `path` |
| `points` | `polyline`, `polygon` |
| `pathLength` (unitless) | `path`, `rect`, `circle`, `ellipse`, `line`, `polyline`, `polygon` |
| `transform` | Root `svg`, `g`, shapes/paths, `text`, `use`, `clipPath` |
| `gradientTransform`, `gradientUnits`, `spreadMethod` | `linearGradient`, `radialGradient` |
| `clipPathUnits` | `clipPath` |
| `offset` | `stop` |

Do not put geometry attributes on other elements: browsers may silently ignore them and produce a different visual from the one intended.

## Content and presentation rules

`svg`, `g`, and `defs` contain the supported graphics, definitions, gradients, clips, styles, titles and descriptions. Gradients contain `stop`, `title`, and `desc`. Text and spans contain text, nested `tspan`, `title`, and `desc`. Clips contain shapes/paths, text, `use`, `title`, and `desc`; groups are not supported inside clips. Shapes, `use`, and stops contain only `title` or `desc`. Titles, descriptions, and styles contain text only. Non-whitespace text outside text/span/style/title/description elements is rejected. In particular, putting a rectangle inside a gradient or a text element is an error.

Presentation attributes and inline styles must apply to their element. `stop-color` and `stop-opacity` belong on stops; `fill` and `stroke` do not belong on stops. Font and text-layout properties belong on text/spans, with inherited properties also accepted on `svg`, `g`, `defs`, `clipPath`, and `use` as inheritance carriers. Paint/stroke properties similarly support graphics and inheritance carriers. `alignment-baseline` is supported on `tspan`; `overflow` on the root `svg`; `shape-rendering` on shapes or inheritance carriers; `vector-effect` on shapes, text, and `use`. Transform a text element or enclosing group rather than a `tspan`.

Stylesheet applicability is checked for each selector and declaration. A selector that matches elements must include a compatible target; an element-name selector must name a compatible element even if none is present. Universal and shared-class rules may also match elements to which a property does not apply, provided the rule has a compatible target. This preserves ordinary rules such as Matplotlib's `* {stroke-linejoin: round}` while rejecting `rect {stop-color: red}` and `.stops {fill: red}` when that class selects only stops. Unmatched class/ID rules are permitted. These checks validate the supported profile; they do not claim that every rule visibly changes the rendered chart.

The profile follows the [SVG property applicability and inheritance model](https://www.w3.org/TR/SVG11/propidx.html) and restricts accepted combinations explicitly rather than relying on a browser to discard unsupported declarations.

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

Each selector contains exactly one universal selector, supported element name, class, or ID; compound selectors such as `rect.label` and `.a.b` are rejected. Selector class/ID names start with an ASCII letter or underscore, optionally preceded by one hyphen, then contain only ASCII letters, digits, underscores or hyphens. Dots inside an SVG ID remain allowed for fragment references, but those IDs cannot be selected in this CSS subset.
