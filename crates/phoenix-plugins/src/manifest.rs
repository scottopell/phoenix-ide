//! `plugin.json` manifest validation (REQ-AP-003, REQ-AP-012).
//!
//! The specification's semantics are deliberately asymmetric:
//! metadata-validity leniency (no SemVer/URL/email/SPDX enforcement),
//! report-and-ignore for unknown top-level fields and a non-object
//! `extensions`, and whole-plugin rejection for everything else. This
//! module implements exactly that; the field walk is manual because a
//! derived struct can neither report unknown fields nor distinguish
//! "ignored" from "rejected".

use serde_json::Value;

/// The canonical 1.1.0 manifest schema identifier (REQ-AP-012).
pub const MANIFEST_SCHEMA_URL: &str = "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json";

/// A validated manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// Required, pattern-checked name.
    pub name: String,
    /// Optional; any string is accepted (`SemVer` recommended, not enforced).
    pub version: Option<String>,
    pub description: Option<String>,
    pub author: Option<Author>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub keywords: Option<Vec<String>>,
    /// Reported-and-ignored when present but not an object; namespaces are
    /// ignored without validating contents (REQ-AP-003, REQ-AP-011).
    pub extensions_ignored: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Author {
    pub name: Option<String>,
    pub email: Option<String>,
    pub url: Option<String>,
}

/// Manifest name validity: 1–64 chars of lowercase ASCII alphanumerics,
/// hyphens, and periods; alphanumeric first/last; no `--` and no `..`
/// (the specification's exact pattern, minus the lookahead, checked
/// directly).
#[must_use]
pub fn name_is_valid(name: &str) -> bool {
    let len = name.chars().count();
    if !(1..=64).contains(&len) {
        return false;
    }
    let mut prev_hyphen_run = false;
    let mut prev_period_run = false;
    for (index, ch) in name.chars().enumerate() {
        let is_alnum = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        if index == 0 || index == len - 1 {
            if !is_alnum {
                return false;
            }
            prev_hyphen_run = false;
            prev_period_run = false;
            continue;
        }
        let is_ok = is_alnum || ch == '-' || ch == '.';
        if !is_ok {
            return false;
        }
        let this_hyphen = ch == '-';
        let this_period = ch == '.';
        if (this_hyphen && prev_hyphen_run) || (this_period && prev_period_run) {
            return false;
        }
        prev_hyphen_run = this_hyphen;
        prev_period_run = this_period;
    }
    true
}

impl Manifest {
    /// Parse and validate manifest JSON (REQ-AP-003).
    ///
    /// # Errors
    /// Returns a rejection for any core violation: non-object root, missing
    /// or invalid `$schema`/`name`, or a type violation of any field.
    /// Unknown top-level fields and a non-object `extensions` are reported
    /// (tracing) and ignored, not rejected.
    pub fn parse(content: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(content)
            .map_err(|e| format!("plugin.json is not valid JSON: {e}"))?;
        Self::from_value(&root)
    }

    /// Validate a parsed manifest document (REQ-AP-003).
    ///
    /// # Errors
    /// Returns a rejection for any core violation: non-object root, missing
    /// or invalid `$schema`/`name`, or a type violation of any field.
    /// Unknown top-level fields and a non-object `extensions` are reported
    /// (tracing) and ignored, not rejected.
    fn from_value(root: &Value) -> Result<Self, String> {
        let Some(object) = root.as_object() else {
            return Err("plugin.json root is not a JSON object".to_string());
        };

        let schema = object
            .get("$schema")
            .and_then(Value::as_str)
            .ok_or_else(|| "plugin.json missing required $schema".to_string())?;
        if schema != MANIFEST_SCHEMA_URL {
            return Err(format!("unsupported plugin.json $schema: {schema}"));
        }

        let name = object
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "plugin.json missing required name".to_string())?;
        if !name_is_valid(name) {
            return Err(format!("plugin.json name is invalid: {name}"));
        }

        let version = optional_string(object, "version")?;
        let description = optional_string(object, "description")?;
        let homepage = optional_string(object, "homepage")?;
        let repository = optional_string(object, "repository")?;
        let license = optional_string(object, "license")?;

        let author = parse_author(object)?;
        let keywords = parse_keywords(object)?;

        let mut extensions_ignored = false;
        if let Some(extensions) = object.get("extensions") {
            if !extensions.is_object() {
                tracing::warn!("plugin.json extensions is not an object; ignoring the field");
                extensions_ignored = true;
            }
            // Implemented namespaces: none in v1 — all namespaces are
            // ignored without validating contents (REQ-AP-011).
        }

        let known = [
            "$schema",
            "name",
            "version",
            "description",
            "author",
            "homepage",
            "repository",
            "license",
            "keywords",
            "extensions",
        ];
        for key in object.keys() {
            if !known.contains(&key.as_str()) {
                // Unknown fields carry no semantics; report and continue
                // (REQ-AP-003).
                tracing::warn!(
                    field = %key,
                    "plugin.json carries an unknown top-level field; ignoring"
                );
            }
        }

        Ok(Self {
            name: name.to_string(),
            version,
            description,
            author,
            homepage,
            repository,
            license,
            keywords,
            extensions_ignored,
        })
    }
}

fn parse_author(object: &serde_json::Map<String, Value>) -> Result<Option<Author>, String> {
    let Some(value) = object.get("author") else {
        return Ok(None);
    };
    let Some(fields) = value.as_object() else {
        return Err("plugin.json author must be an object".to_string());
    };
    let name = optional_string(fields, "name")?;
    let email = optional_string(fields, "email")?;
    let url = optional_string(fields, "url")?;
    for key in fields.keys() {
        if !matches!(key.as_str(), "name" | "email" | "url") {
            // An extra property in author is a type violation.
            return Err(format!("plugin.json author has unexpected field: {key}"));
        }
    }
    Ok(Some(Author { name, email, url }))
}

fn parse_keywords(object: &serde_json::Map<String, Value>) -> Result<Option<Vec<String>>, String> {
    match object.get("keywords") {
        None => Ok(None),
        Some(Value::Array(items)) => {
            let mut keywords = Vec::with_capacity(items.len());
            for item in items {
                let Some(keyword) = item.as_str() else {
                    return Err("plugin.json keywords must be strings".to_string());
                };
                keywords.push(keyword.to_string());
            }
            Ok(Some(keywords))
        }
        Some(_) => Err("plugin.json keywords must be an array".to_string()),
    }
}

fn optional_string(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<String>, String> {
    match object.get(field) {
        None => Ok(None),
        Some(Value::Null) => Err(format!(
            "plugin.json {field} must be a string when present (null is a type violation)"
        )),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("plugin.json {field} must be a string")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"{
        "$schema": "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json",
        "name": "minimal-plugin"
    }"#;

    #[test]
    fn parses_the_specification_minimal_example() {
        let manifest = Manifest::parse(VALID).unwrap();
        assert_eq!(manifest.name, "minimal-plugin");
        assert_eq!(manifest.version, None);
    }

    #[test]
    fn parses_the_specification_full_example() {
        let manifest = Manifest::parse(
            r#"{
                "$schema": "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json",
                "name": "plugin-name",
                "version": "1.2.0",
                "description": "Brief plugin description",
                "author": {"name": "Author Name", "email": "author@example.com", "url": "https://example.com"},
                "homepage": "https://docs.example.com/plugin",
                "repository": "https://github.com/example/plugin",
                "license": "MIT",
                "keywords": ["keyword1", "keyword2"],
                "extensions": {"com.example.client": {"setting": true}}
            }"#,
        )
        .unwrap();
        assert_eq!(manifest.name, "plugin-name");
        assert_eq!(manifest.version.as_deref(), Some("1.2.0"));
        assert_eq!(
            manifest.author.as_ref().and_then(|a| a.name.as_deref()),
            Some("Author Name")
        );
        assert!(!manifest.extensions_ignored);
    }

    #[test]
    fn rejects_missing_or_unsupported_schema() {
        assert!(Manifest::parse(r#"{"name": "x"}"#).is_err());
        assert!(Manifest::parse(
            r#"{"$schema": "https://agent-plugins.org/schemas/2.0.0/plugin.schema.json", "name": "x"}"#
        )
        .is_err());
    }

    #[test]
    fn rejects_invalid_names_and_accepts_valid_ones() {
        for valid in [
            "a",
            "devtools",
            "my.plugin",
            "a-b-c",
            "x9",
            "com.example.long-name.with.periods",
        ] {
            assert!(name_is_valid(valid), "should accept {valid}");
        }
        for invalid in [
            "", "A", "-a", "a-", ".a", "a.", "a--b", "a..b", "a_b", "a b", "UPPER", "ü",
        ] {
            assert!(!name_is_valid(invalid), "should reject {invalid:?}");
        }
        let too_long = "x".repeat(65);
        assert!(!name_is_valid(&too_long));
    }

    #[test]
    fn reports_and_ignores_unknown_top_level_fields() {
        let manifest = Manifest::parse(
            r#"{
                "$schema": "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json",
                "name": "with-extras",
                "mcpServers": {},
                "enabled": true,
                "specVersion": "2"
            }"#,
        )
        .unwrap();
        assert_eq!(manifest.name, "with-extras");
    }

    #[test]
    fn non_object_extensions_is_reported_and_ignored() {
        let manifest = Manifest::parse(
            r#"{
                "$schema": "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json",
                "name": "bad-extensions",
                "extensions": ["not", "an", "object"]
            }"#,
        )
        .unwrap();
        assert!(manifest.extensions_ignored);
    }

    #[test]
    fn metadata_validity_is_lenient_but_types_are_strict() {
        // Not SemVer, not a URL, not an email, not SPDX: all accepted.
        Manifest::parse(
            r#"{
                "$schema": "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json",
                "name": "loose-meta",
                "version": "not-semver",
                "homepage": "not a url",
                "license": "not-spdx"
            }"#,
        )
        .unwrap();

        // Type violations reject.
        for bad in [
            r#"{"$schema": "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json", "name": "x", "version": 1}"#,
            r#"{"$schema": "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json", "name": "x", "description": null}"#,
            r#"{"$schema": "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json", "name": "x", "keywords": [1]}"#,
            r#"{"$schema": "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json", "name": "x", "author": {"name": "a", "extra": 1}}"#,
            r#"{"$schema": "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json", "name": "x", "author": "flat"}"#,
        ] {
            assert!(Manifest::parse(bad).is_err(), "should reject {bad}");
        }
    }

    #[test]
    fn null_for_an_optional_string_is_a_type_violation() {
        let bad = Manifest::parse(
            r#"{
                "$schema": "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json",
                "name": "nulled",
                "version": null
            }"#,
        );
        assert!(bad.is_err());
    }
}
