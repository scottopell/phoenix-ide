use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use roxmltree::{Document, Node, ParsingOptions};
use svgtypes::{Length, LengthUnit, Number, NumberListParser, PathParser, PathSegment, Transform};

mod invocation;
pub use invocation::SvgInvocationId;

pub const MAX_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ELEMENTS: usize = 20_000;
pub const MAX_DEPTH: usize = 64;
pub const MAX_ATTRIBUTES: usize = 100_000;
pub const MAX_PATH_SEGMENTS: usize = 100_000;
pub const MAX_REFERENCES: usize = 10_000;
pub const MAX_EXPANDED_COMPLEXITY: usize = 500_000;
pub const MAX_DIMENSION: f64 = 16_384.0;
pub const MIN_DIMENSION: f64 = 0.000_001;
pub const MAX_PIXEL_AREA: f64 = 64_000_000.0;
pub const MAX_NUMBER: f64 = 10_000_000.0;
pub const MAX_CSS_RULES: usize = 256;
pub const MAX_CSS_SELECTORS: usize = 256;
pub const MAX_CSS_DECLARATIONS: usize = 1_024;
const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

#[derive(Debug)]
pub struct ValidatedSvg {
    bytes: Vec<u8>,
    width: f64,
    height: f64,
}

impl ValidatedSvg {
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    #[must_use]
    pub fn width(&self) -> f64 {
        self.width
    }
    #[must_use]
    pub fn height(&self) -> f64 {
        self.height
    }
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationCategory {
    InvalidInput,
    Policy,
    Limit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidationError {
    pub category: ValidationCategory,
    pub message: &'static str,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}
impl std::error::Error for ValidationError {}

type Result<T> = std::result::Result<T, ValidationError>;

enum ReferenceKind {
    Paint,
    Clip,
    Gradient,
    Reuse,
    ClipReuse,
}

struct LocalReference {
    id: String,
    kind: ReferenceKind,
}

impl LocalReference {
    fn accepts(&self, element: &str) -> bool {
        match self.kind {
            ReferenceKind::Paint | ReferenceKind::Gradient => {
                matches!(element, "linearGradient" | "radialGradient")
            }
            ReferenceKind::Clip => element == "clipPath",
            ReferenceKind::ClipReuse => is_shape(element) || element == "text",
            ReferenceKind::Reuse => matches!(
                element,
                "g" | "path"
                    | "rect"
                    | "circle"
                    | "ellipse"
                    | "line"
                    | "polyline"
                    | "polygon"
                    | "text"
                    | "use"
            ),
        }
    }
}
fn invalid(message: &'static str) -> ValidationError {
    ValidationError {
        category: ValidationCategory::InvalidInput,
        message,
    }
}
fn policy(message: &'static str) -> ValidationError {
    ValidationError {
        category: ValidationCategory::Policy,
        message,
    }
}
fn limit(message: &'static str) -> ValidationError {
    ValidationError {
        category: ValidationCategory::Limit,
        message,
    }
}

/// Validates an SVG snapshot without changing its bytes.
///
/// # Errors
/// Returns a bounded error for malformed XML, unsupported content, or exceeded resource limits.
#[allow(
    clippy::too_many_lines,
    reason = "Keeps the attribute whitelist and graph construction together for security review."
)]
pub fn validate(bytes: &[u8]) -> Result<ValidatedSvg> {
    if bytes.len() > MAX_BYTES {
        return Err(limit(
            "SVG exceeds the 2 MiB byte limit; simplify the chart.",
        ));
    }
    let source = std::str::from_utf8(bytes).map_err(|_| invalid("SVG must be UTF-8 XML."))?;
    validate_declaration(source)?;
    let document = Document::parse_with_options(
        source,
        ParsingOptions {
            allow_dtd: false,
            nodes_limit: 50_000,
        },
    )
    .map_err(|error| match error {
        roxmltree::Error::DtdDetected => {
            policy("DOCTYPE/entities are unsupported; export without a DOCTYPE.")
        }
        roxmltree::Error::NodesLimitReached => {
            limit("SVG exceeds the XML node limit; simplify the chart.")
        }
        roxmltree::Error::InvalidXmlPrefixUri(_)
        | roxmltree::Error::UnexpectedXmlUri(_)
        | roxmltree::Error::UnexpectedXmlnsUri(_)
        | roxmltree::Error::InvalidElementNamePrefix(_)
        | roxmltree::Error::DuplicatedNamespace(..)
        | roxmltree::Error::UnknownNamespace(..)
        | roxmltree::Error::UnexpectedCloseTag(..)
        | roxmltree::Error::UnexpectedEntityCloseTag(_)
        | roxmltree::Error::UnknownEntityReference(..)
        | roxmltree::Error::MalformedEntityReference(_)
        | roxmltree::Error::EntityReferenceLoop(_)
        | roxmltree::Error::InvalidAttributeValue(_)
        | roxmltree::Error::DuplicatedAttribute(..)
        | roxmltree::Error::NoRootNode
        | roxmltree::Error::UnclosedRootNode
        | roxmltree::Error::UnexpectedDeclaration(_)
        | roxmltree::Error::AttributesLimitReached
        | roxmltree::Error::NamespacesLimitReached
        | roxmltree::Error::InvalidName(_)
        | roxmltree::Error::NonXmlChar(..)
        | roxmltree::Error::InvalidChar(..)
        | roxmltree::Error::InvalidChar2(..)
        | roxmltree::Error::InvalidString(..)
        | roxmltree::Error::InvalidExternalID(_)
        | roxmltree::Error::InvalidComment(_)
        | roxmltree::Error::InvalidCharacterData(_)
        | roxmltree::Error::UnknownToken(_)
        | roxmltree::Error::UnexpectedEndOfStream => {
            invalid("Malformed SVG XML; regenerate a well-formed UTF-8 SVG.")
        }
    })?;
    let root = document.root_element();
    if root.tag_name().name() != "svg" || root.tag_name().namespace() != Some(SVG_NS) {
        return Err(invalid(
            "Root must be svg in the http://www.w3.org/2000/svg namespace.",
        ));
    }
    let (width, height) = dimensions(root)?;
    let elements: Vec<_> = document.descendants().filter(Node::is_element).collect();
    if elements.len() > MAX_ELEMENTS {
        return Err(limit("SVG exceeds 20,000 elements; simplify the chart."));
    }
    let index: HashMap<_, _> = elements
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id(), i))
        .collect();
    let mut ids = HashMap::new();
    let mut references = Vec::new();
    let mut edges = vec![Vec::new(); elements.len()];
    let mut weights = vec![1_usize; elements.len()];
    let mut transforms = vec![Transform::default(); elements.len()];
    let mut attributes = 0;
    let mut path_segments = 0;
    let mut css_budget = [0_usize; 3];
    for node in document.descendants() {
        if node.is_pi() {
            return Err(policy("XML processing instructions are unsupported."));
        }
    }
    for (i, node) in elements.iter().copied().enumerate() {
        if node.ancestors().take(MAX_DEPTH + 2).count() > MAX_DEPTH + 1 {
            return Err(limit("SVG nesting exceeds 64 levels; flatten groups."));
        }
        if node.tag_name().namespace() != Some(SVG_NS) || !supported_element(node.tag_name().name())
        {
            return Err(policy(
                "Unsupported SVG element or namespace; use only documented static shapes, text, definitions, clips and gradients.",
            ));
        }
        if i != 0 && node.tag_name().name() == "svg" {
            return Err(policy(
                "Nested svg viewports are unsupported; use groups instead.",
            ));
        }
        content_model(node)?;
        for child in node.children().filter(Node::is_element) {
            edges[i].push(index[&child.id()]);
        }
        if matches!(node.tag_name().name(), "text" | "tspan") {
            weights[i] += node
                .children()
                .filter_map(|n| n.text())
                .map(str::len)
                .sum::<usize>();
        }
        if matches!(node.tag_name().name(), "style" | "title" | "desc")
            && node.children().any(|n| n.is_element())
        {
            return Err(invalid("Style, title and desc must contain text only."));
        }
        if node.tag_name().name() == "style" {
            let text: String = node.children().filter_map(|n| n.text()).collect();
            stylesheet(&text, &mut css_budget, &elements)?;
        }
        for attr in node.attributes() {
            attributes += 1;
            if attributes > MAX_ATTRIBUTES || node.attributes().len() > 64 {
                return Err(limit(
                    "SVG exceeds the attribute limit (64 per element, 100,000 total).",
                ));
            }
            if attr.value().len() > 512_000 {
                return Err(limit(
                    "SVG attribute exceeds 512,000 bytes; split or simplify paths.",
                ));
            }
            let name = attr.name();
            let value = attr.value().trim();
            geometry_attribute(node.tag_name().name(), name)?;
            if attr.namespace().is_some() && !(attr.namespace() == Some(XLINK_NS) && name == "href")
            {
                return Err(policy(
                    "Unsupported attribute namespace; only xlink:href local references are supported.",
                ));
            }
            match name {
                "id" => {
                    identifier(attr.value())?;
                    if ids.insert(value, i).is_some() {
                        return Err(invalid("SVG IDs must be unique."));
                    }
                }
                "class" => {
                    for word in value.split_ascii_whitespace() {
                        identifier(word)?;
                    }
                }
                "version" if i == 0 => keyword(value, &["1.0", "1.1", "2.0"])?,
                "baseProfile" if i == 0 => keyword(value, &["full", "basic", "tiny"])?,
                "viewBox" if i == 0 => {
                    view_box(value)?;
                }
                "preserveAspectRatio" if i == 0 => {
                    let words: Vec<_> = value.split_ascii_whitespace().collect();
                    if words.is_empty() || words.len() > 2 {
                        return Err(invalid("Invalid preserveAspectRatio."));
                    }
                    keyword(
                        words[0],
                        &[
                            "none", "xMinYMin", "xMidYMin", "xMaxYMin", "xMinYMid", "xMidYMid",
                            "xMaxYMid", "xMinYMax", "xMidYMax", "xMaxYMax",
                        ],
                    )?;
                    if words.len() == 2 {
                        keyword(words[1], &["meet", "slice"])?;
                    }
                }
                "href"
                    if matches!(
                        node.tag_name().name(),
                        "use" | "linearGradient" | "radialGradient"
                    ) =>
                {
                    references.push((
                        i,
                        LocalReference {
                            id: fragment(value)?,
                            kind: if node.tag_name().name() == "use" {
                                if node
                                    .parent_element()
                                    .is_some_and(|parent| parent.tag_name().name() == "clipPath")
                                {
                                    ReferenceKind::ClipReuse
                                } else {
                                    ReferenceKind::Reuse
                                }
                            } else {
                                ReferenceKind::Gradient
                            },
                        },
                    ));
                }
                "d" if node.tag_name().name() == "path" => {
                    let count = path(value)?;
                    path_segments += count;
                    weights[i] += count;
                    if path_segments > MAX_PATH_SEGMENTS {
                        return Err(limit("SVG exceeds 100,000 path segments."));
                    }
                }
                "points" if matches!(node.tag_name().name(), "polyline" | "polygon") => {
                    let points = numbers(value)?;
                    if points.len() % 2 != 0 {
                        return Err(invalid("Points must contain coordinate pairs."));
                    }
                    path_segments += points.len() / 2;
                    weights[i] += points.len() / 2;
                    if path_segments > MAX_PATH_SEGMENTS {
                        return Err(limit("SVG exceeds 100,000 path segments."));
                    }
                }
                "transform" | "gradientTransform" => {
                    transforms[i] = compose(transforms[i], transform(value)?)?;
                }
                "x" | "y" | "dx" | "dy" | "x1" | "y1" | "x2" | "y2" | "cx" | "cy" | "fx" | "fy" => {
                    length(value, false)?;
                }
                "width" | "height" | "rx" | "ry" | "r" | "fr" => {
                    length(value, true)?;
                }
                "pathLength" => {
                    if number(value)? < 0.0 {
                        return Err(invalid("pathLength must be a nonnegative unitless number."));
                    }
                }
                "gradientUnits" | "clipPathUnits" => {
                    keyword(value, &["userSpaceOnUse", "objectBoundingBox"])?;
                }
                "spreadMethod" => keyword(value, &["pad", "reflect", "repeat"])?,
                "offset" => {
                    unit_interval(value)?;
                }
                "type" if node.tag_name().name() == "style" => keyword(value, &["text/css"])?,
                "style" => {
                    for (key, val) in declarations(value)? {
                        check_presentation_element(node.tag_name().name(), key)?;
                        if let Some(reference) = presentation(key, val)? {
                            references.push((i, reference));
                        }
                    }
                }
                _ => {
                    check_presentation_element(node.tag_name().name(), name)?;
                    if let Some(reference) = presentation(name, value)? {
                        references.push((i, reference));
                    }
                }
            }
        }
    }
    if references.len() > MAX_REFERENCES {
        return Err(limit("SVG exceeds 10,000 local references."));
    }
    for (from, reference) in references {
        let to = *ids
            .get(reference.id.as_str())
            .ok_or_else(|| invalid("Local SVG reference has no matching ID."))?;
        if !reference.accepts(elements[to].tag_name().name()) {
            return Err(policy(
                "Local reference targets the wrong element type: paints and gradient href require a gradient, clipping requires clipPath, and use requires a renderable element.",
            ));
        }
        edges[from].push(to);
    }
    let mut memo = vec![None; elements.len()];
    let mut active = vec![false; elements.len()];
    complexity(0, &edges, &weights, &mut memo, &mut active, 0)?;
    transformed_geometry(0, &edges, &transforms, Transform::default())?;
    Ok(ValidatedSvg {
        bytes: bytes.to_vec(),
        width,
        height,
    })
}

fn validate_declaration(source: &str) -> Result<()> {
    if let Some(token) = xmlparser::Tokenizer::from(source).next() {
        let token =
            token.map_err(|_| invalid("Malformed SVG XML declaration or opening token."))?;
        if let xmlparser::Token::Declaration {
            version, encoding, ..
        } = token
        {
            if version.as_str() != "1.0" {
                return Err(policy(
                    "Only XML version 1.0 is supported; regenerate as UTF-8 XML 1.0.",
                ));
            }
            if encoding.is_some_and(|encoding| !encoding.as_str().eq_ignore_ascii_case("UTF-8")) {
                return Err(policy(
                    "SVG XML declarations must use UTF-8 encoding, matching the file bytes.",
                ));
            }
        }
    }
    Ok(())
}

fn supported_element(name: &str) -> bool {
    matches!(
        name,
        "svg"
            | "g"
            | "defs"
            | "path"
            | "rect"
            | "circle"
            | "ellipse"
            | "line"
            | "polyline"
            | "polygon"
            | "text"
            | "tspan"
            | "title"
            | "desc"
            | "style"
            | "use"
            | "clipPath"
            | "linearGradient"
            | "radialGradient"
            | "stop"
    )
}

fn geometry_attribute(element: &str, attribute: &str) -> Result<()> {
    let supported = match attribute {
        "transform" => matches!(
            element,
            "svg"
                | "g"
                | "path"
                | "rect"
                | "circle"
                | "ellipse"
                | "line"
                | "polyline"
                | "polygon"
                | "text"
                | "use"
                | "clipPath"
        ),
        "x" | "y" => matches!(element, "rect" | "text" | "tspan" | "use"),
        "dx" | "dy" => matches!(element, "text" | "tspan"),
        "x1" | "y1" | "x2" | "y2" => matches!(element, "line" | "linearGradient"),
        "cx" | "cy" => matches!(element, "circle" | "ellipse" | "radialGradient"),
        "fx" | "fy" | "fr" => element == "radialGradient",
        "width" | "height" => matches!(element, "svg" | "rect"),
        "rx" | "ry" => matches!(element, "rect" | "ellipse"),
        "r" => matches!(element, "circle" | "radialGradient"),
        "pathLength" => matches!(
            element,
            "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon"
        ),
        "gradientTransform" | "gradientUnits" | "spreadMethod" => {
            matches!(element, "linearGradient" | "radialGradient")
        }
        "clipPathUnits" => element == "clipPath",
        "offset" => element == "stop",
        _ => true,
    };
    if supported {
        Ok(())
    } else {
        Err(policy(
            "Geometry attribute is unsupported on this element; use the documented per-element geometry attributes.",
        ))
    }
}

fn is_shape(element: &str) -> bool {
    matches!(
        element,
        "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon"
    )
}

fn content_model(node: Node<'_, '_>) -> Result<()> {
    let parent = node.tag_name().name();
    for child in node.children() {
        if child.is_text()
            && !matches!(parent, "text" | "tspan" | "style" | "title" | "desc")
            && child.text().is_some_and(|text| !text.trim().is_empty())
        {
            return Err(policy(
                "Visible text must be inside text or tspan elements.",
            ));
        }
        if !child.is_element() {
            continue;
        }
        let name = child.tag_name().name();
        let descriptive = matches!(name, "title" | "desc");
        let allowed = match parent {
            "svg" | "g" | "defs" => {
                supported_element(name) && !matches!(name, "svg" | "stop" | "tspan")
            }
            "linearGradient" | "radialGradient" => descriptive || name == "stop",
            "text" | "tspan" => descriptive || name == "tspan",
            "clipPath" => descriptive || is_shape(name) || matches!(name, "text" | "use"),
            "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" | "use"
            | "stop" => descriptive,
            _ => false,
        };
        if !allowed {
            return Err(policy(
                "Unsupported SVG parent-child combination: gradients contain stops, text contains tspan, clips contain shapes/text/use, and shapes/use/stops contain only title or desc.",
            ));
        }
    }
    Ok(())
}

fn presentation_applies(element: &str, property: &str) -> bool {
    let carrier = matches!(element, "svg" | "g" | "defs" | "clipPath" | "use");
    let text = matches!(element, "text" | "tspan");
    let drawing = is_shape(element) || text;
    match property {
        "stop-color" | "stop-opacity" => element == "stop",
        "color" => {
            carrier || drawing || matches!(element, "stop" | "linearGradient" | "radialGradient")
        }
        "fill" | "fill-opacity" | "fill-rule" | "stroke" | "stroke-opacity" | "stroke-width"
        | "stroke-dashoffset" | "stroke-miterlimit" | "stroke-dasharray" | "stroke-linecap"
        | "stroke-linejoin" | "clip-rule" | "visibility" => carrier || drawing,
        "font-size" | "font-family" | "font-style" | "font-weight" | "letter-spacing"
        | "word-spacing" | "text-anchor" | "text-rendering" | "dominant-baseline" => {
            carrier || text
        }
        "alignment-baseline" => element == "tspan",
        "shape-rendering" => carrier || is_shape(element),
        "clip-path" | "opacity" => drawing || matches!(element, "svg" | "g" | "use" | "clipPath"),
        "display" => drawing || matches!(element, "svg" | "g" | "use"),
        "overflow" => element == "svg",
        "vector-effect" => is_shape(element) || matches!(element, "text" | "use"),
        _ => false,
    }
}

fn check_presentation_element(element: &str, property: &str) -> Result<()> {
    if presentation_applies(element, property) {
        Ok(())
    } else {
        Err(policy(
            "Presentation property does not apply to this element in the supported SVG profile; move it to a compatible shape, text, container or gradient stop.",
        ))
    }
}

fn selector_matches(selector: &str, node: Node<'_, '_>) -> bool {
    if selector == "*" {
        true
    } else if let Some(id) = selector.strip_prefix('#') {
        node.attribute("id") == Some(id)
    } else if let Some(class) = selector.strip_prefix('.') {
        node.attribute("class")
            .is_some_and(|classes| classes.split_ascii_whitespace().any(|name| name == class))
    } else {
        node.tag_name().name() == selector
    }
}

fn css_selector(selector: &str) -> Result<()> {
    if selector == "*" {
        return Ok(());
    }
    if let Some(name) = selector.strip_prefix(['.', '#']) {
        let name = name.strip_prefix('-').unwrap_or(name);
        let mut characters = name.bytes();
        if name.len() <= 256
            && characters
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
            && characters.all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            return Ok(());
        }
    } else if supported_element(selector) {
        return Ok(());
    }
    Err(policy(
        "Use a single *, supported element name, .class or #id selector; selector names must start with a letter or underscore (optionally preceded by one hyphen), then contain only letters, digits, underscores or hyphens.",
    ))
}
fn identifier(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 256
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        return Err(policy(
            "IDs and classes must be 1–256 ASCII letters, digits, underscores, dots or hyphens.",
        ));
    }
    Ok(())
}
fn fragment(value: &str) -> Result<String> {
    let id = value.strip_prefix('#').ok_or_else(|| {
        policy("Only local #id references are allowed; no external or embedded resources.")
    })?;
    identifier(id)?;
    Ok(id.to_owned())
}
fn keyword(value: &str, choices: &[&str]) -> Result<()> {
    if choices.contains(&value) {
        Ok(())
    } else {
        Err(policy(
            "Unsupported SVG/CSS keyword; use the documented static styling subset.",
        ))
    }
}
fn bounded(number: f64) -> Result<f64> {
    if !number.is_finite() || number.abs() > MAX_NUMBER {
        Err(limit(
            "SVG numeric values and composed transform coefficients must be finite and within ±10,000,000.",
        ))
    } else {
        Ok(number)
    }
}
fn number(value: &str) -> Result<f64> {
    bounded(
        Number::from_str(value)
            .map_err(|_| invalid("Invalid SVG number."))?
            .0,
    )
}
fn length(value: &str, nonnegative: bool) -> Result<Length> {
    let value = Length::from_str(value)
        .map_err(|_| invalid("Invalid SVG length; use one finite numeric length."))?;
    if matches!(value.unit, LengthUnit::Em | LengthUnit::Ex) {
        return Err(policy(
            "Font-relative em/ex lengths are unsupported; use absolute lengths.",
        ));
    }
    bounded(value.number)?;
    if nonnegative && value.number < 0.0 {
        return Err(invalid("SVG sizes and radii must be nonnegative."));
    }
    Ok(value)
}
fn pixels(value: &str) -> Result<f64> {
    let value = length(value, true)?;
    let scale = match value.unit {
        LengthUnit::None | LengthUnit::Px => 1.0,
        LengthUnit::Pt => 96.0 / 72.0,
        LengthUnit::Pc => 16.0,
        LengthUnit::In => 96.0,
        LengthUnit::Cm => 96.0 / 2.54,
        LengthUnit::Mm => 96.0 / 25.4,
        LengthUnit::Em | LengthUnit::Ex | LengthUnit::Percent => {
            return Err(policy(
                "Root dimensions must use absolute units (px, pt, pc, in, cm, mm), or omit both and provide viewBox.",
            ));
        }
    };
    Ok(value.number * scale)
}
fn numbers(value: &str) -> Result<Vec<f64>> {
    NumberListParser::from(value)
        .map(|v| bounded(v.map_err(|_| invalid("Invalid SVG numeric list."))?))
        .collect()
}
fn view_box(value: &str) -> Result<[f64; 4]> {
    let values: [f64; 4] = numbers(value)?
        .try_into()
        .map_err(|_| invalid("viewBox requires four numbers."))?;
    if values[2] < MIN_DIMENSION || values[3] < MIN_DIMENSION {
        return Err(invalid(
            "viewBox width and height must be at least 0.000001.",
        ));
    }
    Ok(values)
}
fn dimensions(root: Node<'_, '_>) -> Result<(f64, f64)> {
    let view_box = root.attribute("viewBox").map(view_box).transpose()?;
    let (width, height) = match (root.attribute("width"), root.attribute("height")) {
        (Some(w), Some(h)) => (pixels(w)?, pixels(h)?),
        (None, None) => {
            let b = view_box
                .ok_or_else(|| invalid("SVG needs width and height or a positive viewBox."))?;
            (b[2], b[3])
        }
        _ => {
            return Err(invalid(
                "Provide both width and height, or omit both and provide viewBox.",
            ));
        }
    };
    if width < MIN_DIMENSION
        || height < MIN_DIMENSION
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || width * height > MAX_PIXEL_AREA
    {
        return Err(limit(
            "SVG dimensions must be between 0.000001 and 16,384 px per side, with at most 64 million pixels total.",
        ));
    }
    Ok((width, height))
}
fn unit_interval(value: &str) -> Result<()> {
    let n = if let Some(value) = value.strip_suffix('%') {
        number(value)? / 100.0
    } else {
        number(value)?
    };
    if (0.0..=1.0).contains(&n) {
        Ok(())
    } else {
        Err(invalid(
            "Opacity and gradient offsets must be between 0 and 1 (or 0% and 100%).",
        ))
    }
}
fn transform(value: &str) -> Result<Transform> {
    use svgtypes::TransformListToken as Token;
    for (i, token) in svgtypes::TransformListParser::from(value).enumerate() {
        if i >= 64 {
            return Err(limit(
                "At most 64 transform operations are supported per element.",
            ));
        }
        let values = match token.map_err(|_| invalid("Invalid SVG transform."))? {
            Token::Matrix { a, b, c, d, e, f } => [a, b, c, d, e, f],
            Token::Translate { tx, ty } => [tx, ty, 0.0, 0.0, 0.0, 0.0],
            Token::Scale { sx, sy } => [sx, sy, 0.0, 0.0, 0.0, 0.0],
            Token::Rotate { angle } | Token::SkewX { angle } | Token::SkewY { angle } => {
                [angle, 0.0, 0.0, 0.0, 0.0, 0.0]
            }
        };
        for n in values {
            bounded(n)?;
        }
    }
    let t = Transform::from_str(value).map_err(|_| invalid("Invalid SVG transform."))?;
    for n in [t.a, t.b, t.c, t.d, t.e, t.f] {
        bounded(n)?;
    }
    Ok(t)
}

fn compose(parent: Transform, local: Transform) -> Result<Transform> {
    let transform = Transform {
        a: parent.a * local.a + parent.c * local.b,
        b: parent.b * local.a + parent.d * local.b,
        c: parent.a * local.c + parent.c * local.d,
        d: parent.b * local.c + parent.d * local.d,
        e: parent.a * local.e + parent.c * local.f + parent.e,
        f: parent.b * local.e + parent.d * local.f + parent.f,
    };
    for n in [
        transform.a,
        transform.b,
        transform.c,
        transform.d,
        transform.e,
        transform.f,
    ] {
        bounded(n)?;
    }
    Ok(transform)
}

fn transformed_geometry(
    node: usize,
    edges: &[Vec<usize>],
    transforms: &[Transform],
    parent: Transform,
) -> Result<()> {
    let current = compose(parent, transforms[node])?;
    for child in &edges[node] {
        transformed_geometry(*child, edges, transforms, current)?;
    }
    Ok(())
}
fn path(value: &str) -> Result<usize> {
    let mut count = 0;
    for segment in PathParser::from(value) {
        let segment = segment.map_err(|_| invalid("Invalid SVG path data."))?;
        count += 1;
        if count > MAX_PATH_SEGMENTS {
            return Err(limit("SVG exceeds 100,000 path segments."));
        }
        let coordinates: Vec<f64> = match segment {
            PathSegment::MoveTo { x, y, .. }
            | PathSegment::LineTo { x, y, .. }
            | PathSegment::SmoothQuadratic { x, y, .. } => vec![x, y],
            PathSegment::HorizontalLineTo { x, .. } => vec![x],
            PathSegment::VerticalLineTo { y, .. } => vec![y],
            PathSegment::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
                ..
            } => vec![x1, y1, x2, y2, x, y],
            PathSegment::SmoothCurveTo { x2, y2, x, y, .. } => vec![x2, y2, x, y],
            PathSegment::Quadratic { x1, y1, x, y, .. } => vec![x1, y1, x, y],
            PathSegment::EllipticalArc {
                rx,
                ry,
                x_axis_rotation,
                x,
                y,
                ..
            } => {
                if rx < 0.0 || ry < 0.0 {
                    return Err(invalid("Arc radii must be nonnegative."));
                }
                vec![rx, ry, x_axis_rotation, x, y]
            }
            PathSegment::ClosePath { .. } => Vec::new(),
        };
        for n in coordinates {
            bounded(n)?;
        }
    }
    Ok(count)
}

fn declarations(value: &str) -> Result<Vec<(&str, &str)>> {
    if value.contains(['\\', '@', '{', '}', '/', '*', '!', '<', '>']) {
        return Err(policy(
            "CSS escapes, comments, at-rules and !important are unsupported.",
        ));
    }
    value
        .split(';')
        .filter(|v| !v.trim().is_empty())
        .map(|entry| {
            let (key, val) = entry
                .split_once(':')
                .ok_or_else(|| invalid("CSS declarations require property: value."))?;
            Ok((key.trim(), val.trim()))
        })
        .collect()
}
#[allow(
    clippy::too_many_lines,
    reason = "The complete CSS property whitelist is one auditable match."
)]
fn presentation(name: &str, value: &str) -> Result<Option<LocalReference>> {
    match name {
        "fill" | "stroke" | "stop-color" | "color" => {
            if let Some(inner) = value.strip_prefix("url(").and_then(|s| s.strip_suffix(')')) {
                if !matches!(name, "fill" | "stroke") {
                    return Err(policy("This color property cannot reference a resource."));
                }
                return Ok(Some(LocalReference {
                    id: fragment(inner.trim())?,
                    kind: ReferenceKind::Paint,
                }));
            }
            if !matches!(value, "none" | "currentColor" | "inherit") {
                svgtypes::Color::from_str(value).map_err(|_| {
                    policy("Use a static color or a local url(#id) paint reference.")
                })?;
            }
        }
        "clip-path" => {
            if value != "none" {
                let inner = value
                    .strip_prefix("url(")
                    .and_then(|s| s.strip_suffix(')'))
                    .ok_or_else(|| policy("clip-path supports only none or url(#id)."))?;
                return Ok(Some(LocalReference {
                    id: fragment(inner.trim())?,
                    kind: ReferenceKind::Clip,
                }));
            }
        }
        "opacity" | "fill-opacity" | "stroke-opacity" | "stop-opacity" => unit_interval(value)?,
        "font-size" => {
            if length(value, true)?.unit == LengthUnit::Percent {
                return Err(policy(
                    "Percentage font-size is unsupported; use an absolute font size.",
                ));
            }
        }
        "stroke-width" | "stroke-dashoffset" | "letter-spacing" | "word-spacing" => {
            length(
                value,
                name != "stroke-dashoffset" && name != "letter-spacing" && name != "word-spacing",
            )?;
        }
        "stroke-miterlimit" => {
            if number(value)? < 1.0 {
                return Err(invalid("stroke-miterlimit must be at least 1."));
            }
        }
        "stroke-dasharray" => {
            if value != "none" {
                let values = numbers(value)?;
                if values.is_empty() || values.len() > 256 || values.iter().any(|v| *v < 0.0) {
                    return Err(limit(
                        "stroke-dasharray requires 1–256 nonnegative numbers.",
                    ));
                }
            }
        }
        "stroke-linecap" => keyword(value, &["butt", "round", "square"])?,
        "stroke-linejoin" => keyword(value, &["miter", "round", "bevel"])?,
        "fill-rule" | "clip-rule" => keyword(value, &["nonzero", "evenodd"])?,
        "text-anchor" => keyword(value, &["start", "middle", "end"])?,
        "dominant-baseline" | "alignment-baseline" => keyword(
            value,
            &[
                "auto",
                "alphabetic",
                "middle",
                "central",
                "hanging",
                "text-before-edge",
                "text-after-edge",
                "baseline",
            ],
        )?,
        "font-family" => {
            if value.is_empty()
                || value.len() > 256
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b" ,-_'\"".contains(&b))
            {
                return Err(policy("font-family supports simple local font names only."));
            }
        }
        "font-style" => keyword(value, &["normal", "italic", "oblique"])?,
        "font-weight" => keyword(
            value,
            &[
                "normal", "bold", "bolder", "lighter", "100", "200", "300", "400", "500", "600",
                "700", "800", "900",
            ],
        )?,
        "display" => keyword(value, &["none", "inline"])?,
        "visibility" => keyword(value, &["visible", "hidden", "collapse"])?,
        "overflow" => keyword(value, &["hidden", "visible"])?,
        "vector-effect" => keyword(value, &["none", "non-scaling-stroke"])?,
        "shape-rendering" => keyword(
            value,
            &["auto", "optimizeSpeed", "crispEdges", "geometricPrecision"],
        )?,
        "text-rendering" => keyword(
            value,
            &[
                "auto",
                "optimizeSpeed",
                "optimizeLegibility",
                "geometricPrecision",
            ],
        )?,
        _ => {
            return Err(policy(
                "Unsupported SVG attribute or CSS property; remove active content and use the documented static subset.",
            ));
        }
    }
    Ok(None)
}
fn stylesheet(mut value: &str, budget: &mut [usize; 3], elements: &[Node<'_, '_>]) -> Result<()> {
    while !value.trim().is_empty() {
        budget[0] += 1;
        if budget[0] > MAX_CSS_RULES {
            return Err(limit("SVG stylesheets support at most 256 rules."));
        }
        let (selectors, rest) = value
            .split_once('{')
            .ok_or_else(|| invalid("Invalid CSS rule."))?;
        let (body, rest) = rest
            .split_once('}')
            .ok_or_else(|| invalid("Unclosed CSS rule."))?;
        let mut selected_kinds = Vec::new();
        for selector in selectors.trim().split(',') {
            budget[1] += 1;
            if budget[1] > MAX_CSS_SELECTORS {
                return Err(limit(
                    "SVG stylesheets support at most 256 selectors total.",
                ));
            }
            let selector = selector.trim();
            css_selector(selector)?;
            let mut kinds: HashSet<_> = elements
                .iter()
                .copied()
                .filter(|node| selector_matches(selector, *node))
                .map(|node| node.tag_name().name())
                .collect();
            if kinds.is_empty() && supported_element(selector) {
                kinds.insert(selector);
            }
            selected_kinds.push(kinds);
        }
        for (key, val) in declarations(body)? {
            budget[2] += 1;
            if budget[2] > MAX_CSS_DECLARATIONS {
                return Err(limit(
                    "SVG stylesheets support at most 1,024 declarations total.",
                ));
            }
            if presentation(key, val)?.is_some() {
                return Err(policy(
                    "Put local paint and clip references on elements, not in stylesheets.",
                ));
            }
            for kinds in &selected_kinds {
                if !kinds.is_empty()
                    && !kinds
                        .iter()
                        .any(|element| presentation_applies(element, key))
                {
                    return Err(policy(
                        "Stylesheet property has no compatible matched element; target an applicable shape, text, container or gradient stop.",
                    ));
                }
            }
        }
        value = rest;
    }
    Ok(())
}
fn complexity(
    node: usize,
    edges: &[Vec<usize>],
    weights: &[usize],
    memo: &mut [Option<(usize, usize)>],
    active: &mut [bool],
    depth: usize,
) -> Result<(usize, usize)> {
    if depth >= MAX_DEPTH {
        return Err(limit("SVG reference expansion exceeds 64 levels."));
    }
    if active[node] {
        return Err(policy("Cyclic SVG references are forbidden."));
    }
    if let Some((cost, height)) = memo[node] {
        if depth + height > MAX_DEPTH {
            return Err(limit("SVG reference expansion exceeds 64 levels."));
        }
        return Ok((cost, height));
    }
    active[node] = true;
    let mut cost = weights[node];
    let mut height = 1;
    for child in &edges[node] {
        let (child_cost, child_height) =
            complexity(*child, edges, weights, memo, active, depth + 1)?;
        cost = cost.saturating_add(child_cost);
        height = height.max(child_height + 1);
        if cost > MAX_EXPANDED_COMPLEXITY {
            return Err(limit(
                "Expanded SVG complexity exceeds 500,000; reduce repeated definitions or path detail.",
            ));
        }
    }
    active[node] = false;
    memo[node] = Some((cost, height));
    Ok((cost, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    fn svg(body: &str) -> String {
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="80">{body}</svg>"#)
    }
    fn rejected(body: &str) -> ValidationError {
        validate(svg(body).as_bytes()).unwrap_err()
    }

    #[test]
    fn library_export_keeps_paths_glyph_reuse_clips_and_styles_byte_for_byte() {
        let bytes = include_bytes!("fixtures/matplotlib-bars.svg");
        let accepted = validate(bytes).unwrap();
        assert_eq!(accepted.bytes, bytes);
        assert_eq!((accepted.width, accepted.height), (576.0, 288.0));
    }

    #[test]
    fn hand_authored_gradients_clips_text_and_entity_escaped_labels() {
        let source = svg(
            r##"<defs><linearGradient id="paint"><stop offset="0%" stop-color="#fff"/><stop offset="1" stop-color="rgb(10, 20, 30)"/></linearGradient><clipPath id="clip"><rect width="80" height="70"/></clipPath><path id="glyph" d="M0 0 L1 1 Z"/></defs><style>* {stroke-linejoin: round;} .label {font-size: 12px; font-family: 'DejaVu Sans', sans-serif;}</style><g clip-path="url(#clip)"><rect width="100" height="80" fill="url(#paint)"/><use href="#glyph"/><text class="label" x="1" y="20">A &amp; B &#60; C</text></g>"##,
        );
        assert_eq!(
            validate(source.as_bytes()).unwrap().bytes,
            source.as_bytes()
        );
    }

    #[test]
    fn rejects_active_content_namespaces_and_url_encodings() {
        for body in [
            r"<script>alert(1)</script>",
            r#"<rect onload="alert(1)"/>"#,
            r#"<foreignObject><div xmlns="http://www.w3.org/1999/xhtml">HTML</div></foreignObject>"#,
            r#"<a href="https://example.com"><text>link</text></a>"#,
            r#"<animate attributeName="x"/>"#,
            r#"<image href="data:image/png;base64,YQ=="/>"#,
            r#"<use href="https://example.com/a.svg#x"/>"#,
            r#"<use href="&#104;ttps://example.com/a.svg#x"/>"#,
            r#"<use xmlns:q="http://www.w3.org/1999/xlink" q:href="file:///etc/passwd"/>"#,
            r##"<use xmlns:q="https://evil.example" q:href="#local"/>"##,
            r#"<rect fill="url(data:image/svg+xml,evil)"/>"#,
            r#"<rect style="fill: url(https://example.com/x)"/>"#,
            r#"<rect style="fill: u\72l(#x)"/>"#,
            r#"<rect style="fill: red; behavior: url(evil)"/>"#,
            r"<style>@import 'https://example.com';</style>",
            r"<style>* {fill: url(https://example.com)}</style>",
            r"<style>* {fill: u\72l(#x)}</style>",
            r"<style>* {font-family: url(evil)}</style>",
            r#"<rect xml:base="https://example.com"/>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#,
            r#"<?xml-stylesheet href="https://example.com"?>"#,
        ] {
            assert!(validate(svg(body).as_bytes()).is_err(), "accepted {body}");
        }
    }

    #[test]
    fn rejects_dtd_entity_expansion_and_malformed_xml_without_echoing_input() {
        for prefix in [
            r"<!DOCTYPE svg>",
            r#"<!DOCTYPE svg SYSTEM "file:///etc/passwd">"#,
            r#"<!DOCTYPE svg [<!ENTITY a "AAAA"><!ENTITY b "&a;&a;&a;">]>"#,
        ] {
            let error = validate(format!("{prefix}{}", svg("")).as_bytes()).unwrap_err();
            assert_eq!(error.category, ValidationCategory::Policy);
            assert!(!error.message.contains("passwd"));
        }
        for bytes in [&b"<svg"[..], &b"\xff"[..], svg("&unknown;").as_bytes()] {
            assert_eq!(
                validate(bytes).unwrap_err().category,
                ValidationCategory::InvalidInput
            );
        }
    }

    #[test]
    fn rejects_cycles_missing_ids_duplicate_ids_and_exponential_expansion() {
        for body in [
            r##"<use id="x" href="#x"/>"##,
            r##"<g id="x"><use href="#x"/></g>"##,
            r##"<defs><g id="x"><use href="#y"/></g><g id="y"><use href="#x"/></g></defs>"##,
            r#"<rect id="x"/><rect id="x"/>"#,
            r##"<use href="#missing"/>"##,
            r##"<defs><linearGradient id="g" href="#g"/></defs>"##,
        ] {
            assert!(validate(svg(body).as_bytes()).is_err(), "accepted {body}");
        }
        let mut body = String::from(r#"<defs><path id="p0" d="M0 0L1 1"/>"#);
        for i in 1..20 {
            write!(
                body,
                r##"<g id="p{i}"><use href="#p{}"/><use href="#p{}"/></g>"##,
                i - 1,
                i - 1
            )
            .unwrap();
        }
        body.push_str(r##"</defs><use href="#p19"/>"##);
        assert_eq!(rejected(&body).category, ValidationCategory::Limit);
    }

    #[test]
    fn dimensions_and_geometry_are_bounded_and_finite() {
        for pair in [("16384", "1"), ("8000", "8000"), ("1pt", "2pt")] {
            validate(
                format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}"/>"#,
                    pair.0, pair.1
                )
                .as_bytes(),
            )
            .unwrap();
        }
        for pair in [
            ("16385", "1"),
            ("8000", "8001"),
            ("0", "1"),
            ("-1", "1"),
            ("NaN", "1"),
            ("1e999", "1"),
            ("100%", "100%"),
            ("1em", "1em"),
        ] {
            assert!(validate(
                format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}"/>"#,
                    pair.0, pair.1
                )
                .as_bytes()
            )
            .is_err());
        }
        let by_viewbox =
            validate(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 480"/>"#)
                .unwrap();
        assert_eq!((by_viewbox.width, by_viewbox.height), (640.0, 480.0));
        for body in [
            r#"<path d="M1e999 0"/>"#,
            r#"<path d="M0 0 L10000001 0"/>"#,
            r#"<rect x="NaN"/>"#,
            r#"<rect width="-1"/>"#,
            r#"<g transform="scale(1e99)"/>"#,
            r#"<path d="M nope"/>"#,
            r#"<polygon points="1 2 3"/>"#,
        ] {
            assert!(validate(svg(body).as_bytes()).is_err());
        }
    }

    #[test]
    fn byte_element_nesting_and_reference_limits_have_boundaries() {
        let empty = svg("");
        let padding = MAX_BYTES - empty.len() - 7;
        let exact = svg(&format!("<!--{}-->", "x".repeat(padding)));
        assert_eq!(exact.len(), MAX_BYTES);
        validate(exact.as_bytes()).unwrap();
        assert_eq!(
            validate(format!("{exact} ").as_bytes())
                .unwrap_err()
                .category,
            ValidationCategory::Limit
        );
        validate(svg(&"<rect/>".repeat(MAX_ELEMENTS - 1)).as_bytes()).unwrap();
        assert_eq!(
            rejected(&"<rect/>".repeat(MAX_ELEMENTS)).category,
            ValidationCategory::Limit
        );
        validate(
            svg(&format!(
                "{}{}",
                "<g>".repeat(MAX_DEPTH - 1),
                "</g>".repeat(MAX_DEPTH - 1)
            ))
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(
            rejected(&format!(
                "{}{}",
                "<g>".repeat(MAX_DEPTH),
                "</g>".repeat(MAX_DEPTH)
            ))
            .category,
            ValidationCategory::Limit
        );
        let defs = r#"<defs><path id="p" d="M0 0"/></defs>"#;
        validate(
            svg(&format!(
                "{defs}{}",
                r##"<use href="#p"/>"##.repeat(MAX_REFERENCES)
            ))
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(
            rejected(&format!(
                "{defs}{}",
                r##"<use href="#p"/>"##.repeat(MAX_REFERENCES + 1)
            ))
            .category,
            ValidationCategory::Limit
        );
    }

    #[test]
    fn global_path_budget_counts_reused_paths_and_split_geometry() {
        let part = format!(
            r#"<path d="M0 0{}"/>"#,
            "L0 0".repeat(MAX_PATH_SEGMENTS / 2 - 1)
        );
        validate(svg(&part.repeat(2)).as_bytes()).unwrap();
        assert_eq!(
            rejected(&format!("{part}{part}<path d=\"M0 0\"/>")).category,
            ValidationCategory::Limit
        );
        let heavy = format!(
            r#"<defs><path id="p" d="M0 0{}"/></defs>{}"#,
            "L0 0".repeat(90_000),
            r##"<use href="#p"/>"##.repeat(6)
        );
        assert_eq!(rejected(&heavy).category, ValidationCategory::Limit);
    }

    #[test]
    fn stylesheet_and_reused_text_have_global_budgets() {
        let rule = "*{fill:red}";
        validate(svg(&format!("<style>{}</style>", rule.repeat(MAX_CSS_RULES))).as_bytes())
            .unwrap();
        assert_eq!(
            rejected(&format!(
                "<style>{}</style><style>{rule}</style>",
                rule.repeat(MAX_CSS_RULES)
            ))
            .category,
            ValidationCategory::Limit
        );
        assert_eq!(
            rejected(&format!(
                "<style>{}{{fill:red}}</style>",
                vec!["*"; MAX_CSS_SELECTORS + 1].join(",")
            ))
            .category,
            ValidationCategory::Limit
        );
        assert_eq!(
            rejected(&format!(
                "<style>*{{{}}}</style>",
                "fill:red;".repeat(MAX_CSS_DECLARATIONS + 1)
            ))
            .category,
            ValidationCategory::Limit
        );
        let body = format!(
            r#"<defs><text id="t">{}</text></defs>{}"#,
            "A".repeat(100_000),
            r##"<use href="#t"/>"##.repeat(6)
        );
        assert_eq!(rejected(&body).category, ValidationCategory::Limit);
    }

    #[test]
    fn bounds_composed_transforms_in_groups_and_references() {
        validate(svg(r#"<g transform="scale(1000)"><g transform="scale(1000)"><path d="M0 0L1 1"/></g></g>"#).as_bytes()).unwrap();
        for body in [
            r#"<g transform="scale(4000)"><g transform="scale(4000)"><path d="M0 0L1 1"/></g></g>"#,
            r##"<defs><path id="p" transform="scale(4000)" d="M0 0L1 1"/></defs><use href="#p" transform="scale(4000)"/>"##,
            r#"<g transform="scale(1e99) scale(1e-99)"/>"#,
        ] {
            assert_eq!(rejected(body).category, ValidationCategory::Limit);
        }
        assert!(validate(
            br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1e-300 1e-300"/>"#
        )
        .is_err());
    }

    #[test]
    fn xml_declarations_match_utf8_xml_1_0_bytes() {
        for declaration in [
            "",
            "<?xml version=\"1.0\"?>",
            "<?xml version='1.0' encoding='UTF-8'?>",
            "<?xml version = '1.0' encoding = 'utf-8' standalone = 'yes' ?>",
            "<?xml version='1.0' standalone='no'?>",
            "\u{feff}<?xml version='1.0' encoding='UTF-8' standalone='yes'?>",
        ] {
            let source = format!("{declaration}{}", svg("<text>Café &amp; 茶</text>"));
            assert_eq!(
                validate(source.as_bytes()).unwrap().bytes,
                source.as_bytes()
            );
        }
        for declaration in [
            "<?xml version='1.1'?>",
            "<?xml version='1.00'?>",
            "<?xml version='1.0' encoding='UTF-16'?>",
            "<?xml version='1.0' encoding='UTF-16LE'?>",
            "<?xml version='1.0' encoding='ISO-8859-1'?>",
            "<?xml version='1.0' encoding='US-ASCII'?>",
            "\u{feff}<?xml version='1.0' encoding='UTF-16'?>",
        ] {
            assert_eq!(
                validate(format!("{declaration}{}", svg("")).as_bytes())
                    .unwrap_err()
                    .category,
                ValidationCategory::Policy
            );
        }
        for declaration in [
            "<?xml version='2.0'?>",
            "<?xml version='invalid'?>",
            "<?xml version='1.0' standalone='maybe'?>",
            "<?xml version='1.0' encoding='UTF-8' encoding='UTF-16'?>",
            "<?xml version='1.0' encoding='UTF&#45;8'?>",
            "<?xml version='1.0' encoding='UTF-8?>",
        ] {
            assert!(
                validate(format!("{declaration}{}", svg("")).as_bytes()).is_err(),
                "accepted {declaration}"
            );
        }
    }

    #[test]
    fn rejects_font_relative_lengths_and_font_size_growth() {
        for body in [
            r#"<text font-size="2em">text</text>"#,
            r#"<text font-size="2ex">text</text>"#,
            r#"<g font-size="1000000%"><text font-size="1000000%">text</text></g>"#,
            r#"<text style="font-size: 200%">text</text>"#,
            r"<style>* {font-size: 200%;}</style><text>text</text>",
            r#"<rect width="100em" height="100ex"/>"#,
            r#"<text letter-spacing="1em">text</text>"#,
            r#"<rect x="1ex"/>"#,
        ] {
            assert_eq!(rejected(body).category, ValidationCategory::Policy);
        }
        validate(
            svg(r#"<rect width="100%" height="100%"/><text font-size="12pt">text</text>"#)
                .as_bytes(),
        )
        .unwrap();
    }

    #[test]
    fn references_require_compatible_target_elements() {
        for body in [
            r#"<path id="p" d="M0 0L1 1"/><rect width="10" height="10" fill="url(#p)"/>"#,
            r#"<path id="p" d="M0 0L1 1"/><rect width="10" height="10" style="stroke: url(#p)"/>"#,
            r#"<linearGradient id="p"/><rect width="10" height="10" clip-path="url(#p)"/>"#,
            r##"<rect id="p" width="1" height="1"/><linearGradient href="#p"/>"##,
            r##"<clipPath id="p"/><radialGradient href="#p"/>"##,
            r##"<linearGradient id="p"/><use href="#p"/>"##,
            r##"<defs id="p"/><use href="#p"/>"##,
            r##"<style id="p">*{fill:red}</style><use href="#p"/>"##,
        ] {
            assert_eq!(
                rejected(body).category,
                ValidationCategory::Policy,
                "accepted {body}"
            );
        }
        validate(svg(r##"<defs><linearGradient id="a"><stop offset="0" stop-color="red"/></linearGradient><radialGradient id="b" href="#a"/><clipPath id="c"><circle r="3"/></clipPath><g id="g"><rect width="1" height="1"/></g></defs><rect width="10" height="10" fill="url(#b)" style="clip-path: url(#c)"/><use href="#g"/>"##).as_bytes()).unwrap();
    }

    #[test]
    fn stylesheet_selectors_follow_the_exact_simple_grammar() {
        for selector in [
            "*",
            "rect",
            "linearGradient",
            ".label",
            "#chart_1",
            ".-label",
            "#_chart",
            ".a-2",
        ] {
            validate(svg(&format!("<style>{selector}{{color:red}}</style>")).as_bytes()).unwrap();
        }
        for selector in [
            ".",
            "#",
            "..label",
            "##chart",
            "rect.label",
            "#chart.label",
            ".a.b",
            "1rect",
            ".1label",
            "#-1chart",
            ".--label",
            "unknown",
            "g rect",
            "rect:hover",
            "rect>path",
            "*rect",
            ".a,",
            "",
        ] {
            assert_eq!(
                rejected(&format!("<style>{selector}{{fill:red}}</style>")).category,
                ValidationCategory::Policy,
                "accepted {selector}"
            );
        }
        validate(svg("<style>rect, .label, #chart {fill:red}</style>").as_bytes()).unwrap();
    }

    #[test]
    fn geometry_attributes_apply_only_to_their_supported_elements() {
        for body in [
            r#"<circle width="100" height="100"/>"#,
            r#"<rect cx="100"/>"#,
            r#"<ellipse r="5"/>"#,
            r#"<path x="5"/>"#,
            r#"<g dx="5"/>"#,
            r#"<rect x1="5"/>"#,
            r#"<linearGradient fx="5"/>"#,
            r#"<circle fr="5"/>"#,
            r#"<use width="5"/>"#,
            r#"<rect gradientTransform="scale(2)"/>"#,
            r#"<g clipPathUnits="userSpaceOnUse"/>"#,
            r#"<rect gradientUnits="userSpaceOnUse"/>"#,
            r#"<rect spreadMethod="repeat"/>"#,
            r#"<circle offset="0.5"/>"#,
            r#"<text pathLength="3"/>"#,
            r#"<defs transform="scale(2)"/>"#,
        ] {
            assert_eq!(
                rejected(body).category,
                ValidationCategory::Policy,
                "accepted {body}"
            );
        }
        validate(svg(r#"<rect x="1" y="2" width="3" height="4" rx="1" ry="1" pathLength="5"/><circle cx="1" cy="2" r="3"/><ellipse cx="1" cy="2" rx="3" ry="4"/><line x1="1" y1="2" x2="3" y2="4"/><text x="1" y="2" dx="1" dy="2">Label</text><defs><linearGradient x1="0" y1="0" x2="1" y2="1" gradientUnits="objectBoundingBox" gradientTransform="scale(1)" spreadMethod="pad"><stop offset="0.5"/></linearGradient><radialGradient cx="1" cy="2" r="3" fx="1" fy="2" fr="0"/><clipPath clipPathUnits="userSpaceOnUse"><rect width="3" height="4"/></clipPath></defs>"#).as_bytes()).unwrap();
        assert_eq!(
            rejected(r#"<path pathLength="3px"/>"#).category,
            ValidationCategory::InvalidInput
        );
    }

    #[test]
    fn presentation_attributes_and_inline_styles_require_compatible_elements() {
        for body in [
            r#"<rect width="10" height="10" stop-color="red"/>"#,
            r#"<rect style="stop-opacity: 0.5"/>"#,
            r#"<linearGradient><stop fill="red"/></linearGradient>"#,
            r#"<linearGradient><stop style="stroke: red"/></linearGradient>"#,
            r#"<g stop-color="red"><rect/></g>"#,
            r#"<linearGradient style="fill: red"><stop/></linearGradient>"#,
            r#"<rect font-size="12"/>"#,
            r#"<path text-anchor="middle"/>"#,
            r#"<text shape-rendering="crispEdges">text</text>"#,
            r#"<g overflow="hidden"/>"#,
            r#"<linearGradient><stop opacity="0.5"/></linearGradient>"#,
            r#"<text><tspan transform="scale(2)">text</tspan></text>"#,
            r##"<path id=" glyph " d="M0 0L1 1"/><use href="#glyph"/>"##,
        ] {
            assert_eq!(
                rejected(body).category,
                ValidationCategory::Policy,
                "accepted {body}"
            );
        }
        validate(svg(r##"<g fill="red" font-size="12" style="stroke: blue; text-anchor: middle"><rect width="10" height="10"/><text>Label<tspan alignment-baseline="middle">x</tspan></text></g><defs fill="green" font-family="sans-serif"><path id="p" d="M0 0L1 1"/><text id="t">Text</text><linearGradient color="blue"><stop stop-color="currentColor" stop-opacity="0.5"/></linearGradient></defs><use href="#p"/><use href="#t"/>"##).as_bytes()).unwrap();
    }

    #[test]
    fn stylesheet_applicability_checks_selectors_without_rejecting_broad_rules() {
        for body in [
            r"<style>rect {stop-color:red}</style><rect/>",
            r"<style>stop {fill:red}</style><linearGradient><stop/></linearGradient>",
            r#"<style>#wrong {stop-opacity:0.5}</style><rect id="wrong"/>"#,
            r#"<style>.wrong {font-size:12px}</style><rect class="wrong"/>"#,
            r"<style>rect, stop {stop-color:red}</style><rect/><linearGradient><stop/></linearGradient>",
        ] {
            assert_eq!(
                rejected(body).category,
                ValidationCategory::Policy,
                "accepted {body}"
            );
        }
        validate(svg(r#"<style>* {stroke-linejoin:round; fill:red} .mixed {stop-color:blue} .text {font-size:12px} .unused {fill:green}</style><g class="text"><text>Label</text></g><rect class="mixed"/><linearGradient><stop class="mixed"/></linearGradient>"#).as_bytes()).unwrap();
    }

    #[test]
    fn rejects_visual_children_outside_svg_content_models() {
        for body in [
            r#"<linearGradient id="g"><rect width="10" height="10" fill="red"/></linearGradient><rect width="10" height="10" fill="url(#g)"/>"#,
            "<radialGradient><g><stop/></g></radialGradient>",
            "<stop/>",
            "<tspan>orphan</tspan>",
            "<text><rect/></text>",
            "<text><text>nested</text></text>",
            "<text><tspan><path/></tspan></text>",
            "<rect><g><circle/></g></rect>",
            "<use><rect/></use>",
            "<linearGradient><stop><rect/></stop></linearGradient>",
            "<clipPath><g><rect/></g></clipPath>",
            "<clipPath><linearGradient/></clipPath>",
            "<title><text>nested</text></title>",
            "<desc><rect/></desc>",
            "<g>invisible raw text</g>",
            "<linearGradient>ignored text</linearGradient>",
            r##"<g id="g"><rect/></g><clipPath><use href="#g"/></clipPath>"##,
        ] {
            assert!(validate(svg(body).as_bytes()).is_err(), "accepted {body}");
        }
        validate(svg(r##"<title>Title</title><desc>Description</desc><defs><path id="p" d="M0 0L1 1"/><clipPath id="c"><title>Clip</title><use href="#p"/><text>Clip text<tspan>x</tspan></text></clipPath><linearGradient><title>Gradient</title><stop offset="0"><desc>Stop</desc></stop></linearGradient></defs><g><rect width="10" height="10"><title>Rectangle</title></rect><text>Text<tspan>Span<tspan>Nested span</tspan></tspan></text></g>"##).as_bytes()).unwrap();
    }
}
