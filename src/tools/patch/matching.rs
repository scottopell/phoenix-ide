//! Text matching algorithms for finding unique edit locations
//!
//! Implements exact matching with fuzzy fallbacks for common whitespace issues.

use super::types::{EditSpec, PatchError};

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

/// Get leading whitespace from a string
pub fn leading_whitespace(s: &str) -> &str {
    let trimmed = s.trim_start();
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
}
