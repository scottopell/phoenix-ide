use crate::{invalid, ValidationError};

pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_DESCRIPTION_CHARS: usize = 2_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SvgPresentationMetadata {
    title: String,
    description: String,
}

impl SvgPresentationMetadata {
    /// Validates presentation text, preserving accepted strings exactly.
    ///
    /// # Errors
    /// Returns an invalid-input error for whitespace-only text, control characters,
    /// titles exceeding 200 Unicode scalar values, or descriptions exceeding 2,000.
    pub fn new(title: &str, description: &str) -> Result<Self, ValidationError> {
        if !valid_text(title, MAX_TITLE_CHARS) || !valid_text(description, MAX_DESCRIPTION_CHARS) {
            return Err(invalid(
                "Title and description must be nonempty plain text without control characters, at most 200 and 2000 characters respectively.",
            ));
        }
        Ok(Self {
            title: title.to_owned(),
            description: description.to_owned(),
        })
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    #[must_use]
    pub fn into_parts(self) -> (String, String) {
        (self.title, self.description)
    }
}

fn valid_text(value: &str, max: usize) -> bool {
    !value
        .trim_matches(|ch: char| ch.is_whitespace() || ch == '\u{feff}')
        .is_empty()
        && value.chars().count() <= max
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ValidationCategory;

    #[test]
    fn preserves_unicode_and_surrounding_whitespace() {
        let title = "  Café 🦀  ";
        let description = "\u{2003}Measured 使用量 e\u{301}\u{a0}";
        let metadata = SvgPresentationMetadata::new(title, description).unwrap();
        assert_eq!(metadata.title(), title);
        assert_eq!(metadata.description(), description);
        assert_eq!(
            metadata.into_parts(),
            (title.to_owned(), description.to_owned())
        );
    }

    #[test]
    fn counts_unicode_scalars_at_both_boundaries() {
        let title = "🦀".repeat(MAX_TITLE_CHARS);
        let description = "é".repeat(MAX_DESCRIPTION_CHARS);
        SvgPresentationMetadata::new(&title, &description).unwrap();
        assert!(SvgPresentationMetadata::new(&format!("{title}🦀"), &description).is_err());
        assert!(SvgPresentationMetadata::new(&title, &format!("{description}é")).is_err());
        let combining = "e\u{301}".repeat(MAX_TITLE_CHARS / 2);
        SvgPresentationMetadata::new(&combining, "Description").unwrap();
        assert!(SvgPresentationMetadata::new(&format!("{combining}x"), "Description").is_err());
    }

    #[test]
    fn rejects_empty_and_unicode_whitespace_only_fields() {
        for blank in ["", " ", "\u{a0}", "\u{2003}\u{202f}", "\u{3000} "] {
            for (title, description) in [(blank, "Description"), ("Title", blank)] {
                let error = SvgPresentationMetadata::new(title, description).unwrap_err();
                assert_eq!(error.category, ValidationCategory::InvalidInput);
                assert!(error.message.len() < 200);
            }
        }
    }

    #[test]
    fn rejects_embedded_controls_in_each_field() {
        for codepoint in (0..=31).chain(127..=159) {
            let control = char::from_u32(codepoint).unwrap();
            let text = format!("before{control}after");
            assert!(SvgPresentationMetadata::new(&text, "Description").is_err());
            assert!(SvgPresentationMetadata::new("Title", &text).is_err());
        }
    }

    #[test]
    fn rejects_bom_only_text_but_preserves_bom_with_visible_text() {
        assert!(!"\u{feff}".trim().is_empty());
        for blank in ["\u{feff}", " \u{feff}\u{a0}\u{feff} "] {
            assert!(SvgPresentationMetadata::new(blank, "Description").is_err());
            assert!(SvgPresentationMetadata::new("Title", blank).is_err());
        }
        let title = "\u{feff}Title\u{feff}";
        let description = " \u{feff}Description\u{feff} ";
        let metadata = SvgPresentationMetadata::new(title, description).unwrap();
        assert_eq!(metadata.title(), title);
        assert_eq!(metadata.description(), description);
    }
}
