//! Text matching algorithms for finding unique edit locations
//!
//! Implements exact matching with fuzzy fallbacks for common whitespace issues.

use super::types::{EditSpec, PatchError};
use unicode_security::skeleton;

/// Find a unique match for `old_text` in `content`
///
/// Tries in order:
/// 1. Exact match
/// 2. Dedent matching (different indentation levels)
/// 3. Trimmed line matching (first/last line variations)
pub fn find_unique_match(content: &str, old_text: &str) -> Result<EditSpec, PatchError> {
    // 1. Try exact match
    if let Some(spec) = find_exact_unique(content, old_text) {
        return Ok(spec);
    }

    // 2. Try dedent matching
    if let Some(spec) = find_dedent_match(content, old_text) {
        return Ok(spec);
    }

    // 3. Try trimmed line match
    if let Some(spec) = find_trimmed_match(content, old_text) {
        return Ok(spec);
    }

    // 4. Try NFKC-normalised match (handles lookalike characters)
    if let Some(spec) = find_normalised_match(content, old_text) {
        return Ok(spec);
    }

    // Determine error type
    let count = content.matches(old_text).count();
    if count > 1 {
        Err(PatchError::OldTextNotUnique(count))
    } else {
        Err(PatchError::OldTextNotFound)
    }
}

/// Find exact unique match
pub fn find_exact_unique(content: &str, old_text: &str) -> Option<EditSpec> {
    let matches: Vec<_> = content.match_indices(old_text).collect();
    if matches.len() == 1 {
        Some(EditSpec {
            offset: matches[0].0,
            length: old_text.len(),
        })
    } else {
        None
    }
}

/// Find match with different indentation
fn find_dedent_match(content: &str, old_text: &str) -> Option<EditSpec> {
    let old_indent = common_leading_whitespace(old_text);

    // Try different indent levels found in the content
    for line in content.lines() {
        let line_indent = leading_whitespace(line);
        if line_indent != old_indent && !line_indent.is_empty() {
            // Try reindenting old_text to this level
            let adjusted = reindent_text(old_text, &old_indent, line_indent);
            if let Some(spec) = find_exact_unique(content, &adjusted) {
                return Some(spec);
            }
        }
    }
    None
}

/// Find match with trimmed first/last lines
fn find_trimmed_match(content: &str, old_text: &str) -> Option<EditSpec> {
    let lines: Vec<&str> = old_text.lines().collect();
    if lines.len() <= 2 {
        return None;
    }

    // Try without first line
    let without_first = lines[1..].join("\n");
    if let Some(mut spec) = find_exact_unique(content, &without_first) {
        if spec.offset > 0 {
            // Safety: `spec.offset` is from `find_exact_unique()` which returns byte
            // offsets found via `str::find()` on `content`
            #[allow(clippy::string_slice)]
            let before = &content[..spec.offset];
            let first_line_with_newline = format!("{}\n", lines[0]);
            if before.ends_with(&first_line_with_newline) {
                spec.offset -= first_line_with_newline.len();
                spec.length += first_line_with_newline.len();
                return Some(spec);
            }
        }
        return Some(spec);
    }

    // Try without last line
    let without_last = lines[..lines.len() - 1].join("\n");
    if let Some(spec) = find_exact_unique(content, &without_last) {
        return Some(spec);
    }

    None
}

/// Find match using Unicode TR39 confusable skeleton mapping.
///
/// Maps both content and `old_text` to their "skeleton" forms (visually
/// confusable characters collapse to a common representation), then
/// finds the match in skeleton space and maps the offset back to the
/// original content's byte positions.
///
/// Handles lookalike characters: em dash vs hyphen, curly vs straight
/// quotes, fullwidth vs ASCII, etc.
fn find_normalised_match(content: &str, old_text: &str) -> Option<EditSpec> {
    let skel_old: String = skeleton(old_text).collect();

    // If skeleton didn't change old_text, this strategy can't help
    if skel_old == old_text {
        return None;
    }

    // Build skeleton content with a byte-offset map back to original.
    let mut skel_content = String::new();
    let mut skel_to_orig: Vec<usize> = Vec::new();

    for (orig_byte_offset, ch) in content.char_indices() {
        let ch_str = String::from(ch);
        for skel_ch in skeleton(&ch_str) {
            let start = skel_content.len();
            skel_content.push(skel_ch);
            let end = skel_content.len();
            for _ in start..end {
                skel_to_orig.push(orig_byte_offset);
            }
        }
    }
    // Sentinel: map one past the end to content.len()
    skel_to_orig.push(content.len());

    // Find unique match in skeleton content
    let spec = find_exact_unique(&skel_content, &skel_old)?;

    // Map skeleton byte range back to original byte range
    let orig_start = skel_to_orig[spec.offset];
    let orig_end = if spec.offset + spec.length < skel_to_orig.len() {
        skel_to_orig[spec.offset + spec.length]
    } else {
        content.len()
    };

    Some(EditSpec {
        offset: orig_start,
        length: orig_end - orig_start,
    })
}

/// Get leading whitespace from a string
pub fn leading_whitespace(s: &str) -> &str {
    let trimmed = s.trim_start();
    // Safety: `s.len() - trimmed.len()` is the byte length of leading whitespace,
    // which is a valid boundary since `trim_start()` splits at a char boundary
    #[allow(clippy::string_slice)]
    &s[..s.len() - trimmed.len()]
}

/// Get common leading whitespace across all non-empty lines
pub fn common_leading_whitespace(text: &str) -> String {
    let mut common: Option<String> = None;

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let ws = leading_whitespace(line).to_string();
        common = match common {
            None => Some(ws),
            Some(c) => {
                let prefix: String = c
                    .chars()
                    .zip(ws.chars())
                    .take_while(|(a, b)| a == b)
                    .map(|(a, _)| a)
                    .collect();
                Some(prefix)
            }
        };
    }

    common.unwrap_or_default()
}

/// Reindent text from one indentation level to another
pub fn reindent_text(text: &str, old_indent: &str, new_indent: &str) -> String {
    text.lines()
        .map(|line| {
            if line.trim().is_empty() {
                line.to_string()
            } else if let Some(rest) = line.strip_prefix(old_indent) {
                format!("{new_indent}{rest}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let content = "hello world";
        let spec = find_unique_match(content, "world").unwrap();
        assert_eq!(spec.offset, 6);
        assert_eq!(spec.length, 5);
    }

    #[test]
    fn test_no_match() {
        let content = "hello world";
        let err = find_unique_match(content, "foo").unwrap_err();
        assert_eq!(err, PatchError::OldTextNotFound);
    }

    #[test]
    fn test_multiple_matches() {
        let content = "hello hello";
        let err = find_unique_match(content, "hello").unwrap_err();
        assert_eq!(err, PatchError::OldTextNotUnique(2));
    }

    #[test]
    fn test_dedent_match() {
        // Test dedent matching with tab vs space indent
        // Content uses tabs, old_text uses spaces - these won't overlap
        let content = "\t\tindented line";
        let old_text = "  indented line"; // 2-space indent
        let spec = find_unique_match(content, old_text).unwrap();
        // Should find the tab-indented version
        assert_eq!(spec.offset, 0);
        assert_eq!(spec.length, content.len());
    }

    #[test]
    fn test_leading_whitespace() {
        assert_eq!(leading_whitespace("  hello"), "  ");
        assert_eq!(leading_whitespace("hello"), "");
        assert_eq!(leading_whitespace("\t\thello"), "\t\t");
    }

    #[test]
    fn test_common_leading_whitespace() {
        let text = "  line1\n  line2\n  line3";
        assert_eq!(common_leading_whitespace(text), "  ");

        let text2 = "    line1\n  line2"; // Mixed
        assert_eq!(common_leading_whitespace(text2), "  ");
    }

    #[test]
    fn test_reindent_text() {
        let text = "  line1\n  line2";
        let result = reindent_text(text, "  ", "    ");
        assert_eq!(result, "    line1\n    line2");
    }

    #[test]
    fn test_normalised_match_em_dash() {
        // File has em dash, old_text has em dash -- byte-identical, should match exact.
        // But if LLM sends a different dash, normalised match catches it.
        let content = "before \u{2014} after"; // em dash
        let old_text = "before \u{2014} after";
        let spec = find_unique_match(content, old_text).unwrap();
        assert_eq!(spec.offset, 0);
        assert_eq!(spec.length, content.len());
    }

    #[test]
    fn test_normalised_match_curly_quotes() {
        // File has straight quotes, old_text has curly quotes
        let content = r#"say "hello" please"#;
        let old_text = "say \u{201C}hello\u{201D} please"; // curly double quotes
        let spec = find_unique_match(content, old_text).unwrap();
        assert_eq!(spec.offset, 0);
        assert_eq!(spec.length, content.len());
    }

    #[test]
    fn test_normalised_match_ellipsis() {
        // File has three dots, old_text has ellipsis character
        let content = "wait... done";
        let old_text = "wait\u{2026} done"; // ellipsis character
        let spec = find_unique_match(content, old_text).unwrap();
        assert_eq!(spec.offset, 0);
        assert_eq!(spec.length, content.len());
    }

    #[test]
    fn test_normalised_match_offset_correct() {
        // Ensure the returned offset points to the right bytes in original content
        let content = "prefix \u{201C}target\u{201D} suffix";
        let old_text = "\"target\""; // straight quotes in old_text
        let spec = find_unique_match(content, old_text).unwrap();
        #[allow(clippy::string_slice)]
        let matched = &content[spec.offset..spec.offset + spec.length];
        assert_eq!(matched, "\u{201C}target\u{201D}");
    }

    #[test]
    fn test_normalised_no_help_when_text_absent() {
        // Normalisation can't help if the text simply isn't there
        let content = "hello world";
        let err = find_unique_match(content, "goodbye").unwrap_err();
        assert_eq!(err, PatchError::OldTextNotFound);
    }
}
