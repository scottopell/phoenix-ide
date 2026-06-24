//! Text matching algorithms for finding unique edit locations
//!
//! Implements exact matching with fuzzy fallbacks for common whitespace issues.

use super::types::{DuplicateMatchDiagnostics, DuplicateMatchLocation, EditSpec, PatchError};
use unicode_security::skeleton;

const MAX_DUPLICATE_LOCATIONS: usize = 5;
const MAX_SNIPPET_LINES: usize = 5;
const MAX_SNIPPET_CHARS: usize = 240;

#[derive(Debug, Clone, PartialEq, Eq)]
enum MatchOutcome {
    Unique(EditSpec),
    Duplicate(DuplicateMatchDiagnostics),
}

/// Find every non-overlapping exact occurrence of `old_text` in `content`.
///
/// Used by `replace` with `replace_all`: the fuzzy cascade is deliberately not
/// consulted, because "single best candidate" has no meaning across many sites.
/// An empty `old_text` yields no matches (rather than one per byte boundary).
#[must_use]
pub(super) fn find_all_exact(content: &str, old_text: &str) -> Vec<EditSpec> {
    if old_text.is_empty() {
        return Vec::new();
    }
    content
        .match_indices(old_text)
        .map(|(offset, _)| EditSpec {
            offset,
            length: old_text.len(),
        })
        .collect()
}

/// Find a unique match for `old_text` in `content`
///
/// Tries in order:
/// 1. Exact match
/// 2. Dedent matching (different indentation levels)
/// 3. Trimmed line matching (first/last line variations)
/// 4. Unicode confusable-skeleton match (handles lookalike characters)
pub fn find_unique_match(content: &str, old_text: &str) -> Result<EditSpec, PatchError> {
    let mut duplicate: Option<DuplicateMatchDiagnostics> = None;

    if let Some(outcome) = find_exact_match(content, old_text) {
        match outcome {
            MatchOutcome::Unique(spec) => return Ok(spec),
            MatchOutcome::Duplicate(diagnostics) => duplicate = Some(diagnostics),
        }
    }

    if let Some(outcome) = find_dedent_match(content, old_text) {
        match outcome {
            MatchOutcome::Unique(spec) => return Ok(spec),
            MatchOutcome::Duplicate(diagnostics) => duplicate.get_or_insert(diagnostics),
        };
    }

    if let Some(outcome) = find_trimmed_match(content, old_text) {
        match outcome {
            MatchOutcome::Unique(spec) => return Ok(spec),
            MatchOutcome::Duplicate(diagnostics) => duplicate.get_or_insert(diagnostics),
        };
    }

    if let Some(outcome) = find_normalised_match(content, old_text) {
        match outcome {
            MatchOutcome::Unique(spec) => return Ok(spec),
            MatchOutcome::Duplicate(diagnostics) => duplicate.get_or_insert(diagnostics),
        };
    }

    duplicate.map_or(Err(PatchError::OldTextNotFound), |diagnostics| {
        Err(PatchError::OldTextNotUnique(diagnostics))
    })
}

fn find_exact_match(content: &str, old_text: &str) -> Option<MatchOutcome> {
    let mut matches = content.match_indices(old_text);
    let (first_offset, _) = matches.next()?;
    let Some((second_offset, _)) = matches.next() else {
        return Some(MatchOutcome::Unique(EditSpec {
            offset: first_offset,
            length: old_text.len(),
        }));
    };

    Some(MatchOutcome::Duplicate(
        duplicate_match_diagnostics_from_ranges(
            content,
            std::iter::once((first_offset, old_text.len()))
                .chain(std::iter::once((second_offset, old_text.len())))
                .chain(matches.map(|(offset, _)| (offset, old_text.len()))),
        ),
    ))
}

#[cfg(test)]
fn duplicate_match_diagnostics(content: &str, old_text: &str) -> DuplicateMatchDiagnostics {
    duplicate_match_diagnostics_from_ranges(
        content,
        content
            .match_indices(old_text)
            .map(|(offset, _)| (offset, old_text.len())),
    )
}

fn duplicate_match_diagnostics_from_ranges(
    content: &str,
    ranges: impl Iterator<Item = (usize, usize)>,
) -> DuplicateMatchDiagnostics {
    let mut reported = Vec::new();
    let mut total = 0;

    for (offset, len) in ranges {
        total += 1;
        if reported.len() < MAX_DUPLICATE_LOCATIONS {
            reported.push(DuplicateMatchLocation {
                start_line: line_number_at(content, offset),
                snippet: duplicate_match_snippet(content, offset, len),
            });
        }
    }

    DuplicateMatchDiagnostics {
        total,
        omitted: total.saturating_sub(reported.len()),
        reported,
    }
}

fn line_number_at(content: &str, offset: usize) -> usize {
    content
        .char_indices()
        .take_while(|(i, _)| *i < offset)
        .filter(|(_, ch)| *ch == '\n')
        .count()
        + 1
}

fn duplicate_match_snippet(content: &str, offset: usize, len: usize) -> String {
    let match_end = offset + len;
    let start = context_start(content, offset);
    let end = context_end(content, match_end, start);
    let snippet = content.get(start..end).unwrap_or_default();
    truncate_snippet_around_match(snippet, offset - start)
}

fn context_start(content: &str, offset: usize) -> usize {
    let mut start = line_start(content, offset);
    if start > 0 {
        start = line_start(content, start - 1);
    }
    start
}

fn context_end(content: &str, offset: usize, snippet_start: usize) -> usize {
    let mut end = line_end(content, offset);
    if end < content.len() {
        end = line_end(content, end + 1);
    }
    cap_end_to_max_lines(content, snippet_start, end)
}

fn cap_end_to_max_lines(content: &str, start: usize, end: usize) -> usize {
    let mut newline_count = 0;
    for (i, ch) in content.char_indices().skip_while(|(i, _)| *i < start) {
        if i >= end {
            break;
        }
        if ch == '\n' {
            newline_count += 1;
            if newline_count >= MAX_SNIPPET_LINES {
                return i;
            }
        }
    }
    end
}

fn line_start(content: &str, offset: usize) -> usize {
    content
        .char_indices()
        .take_while(|(i, _)| *i < offset)
        .filter_map(|(i, ch)| (ch == '\n').then_some(i + 1))
        .last()
        .unwrap_or(0)
}

fn line_end(content: &str, offset: usize) -> usize {
    content
        .char_indices()
        .find_map(|(i, ch)| (i >= offset && ch == '\n').then_some(i))
        .unwrap_or(content.len())
}

fn truncate_snippet_around_match(snippet: &str, match_offset: usize) -> String {
    if snippet.chars().count() <= MAX_SNIPPET_CHARS {
        return snippet.to_string();
    }

    let half_window = MAX_SNIPPET_CHARS / 2;
    let match_char_offset = snippet
        .char_indices()
        .take_while(|(i, _)| *i < match_offset)
        .count();
    let snippet_chars = snippet.chars().count();
    let start_char = match_char_offset.saturating_sub(half_window);
    let end_char = (start_char + MAX_SNIPPET_CHARS).min(snippet_chars);

    let mut result = String::new();
    if start_char > 0 {
        result.push('…');
    }
    result.extend(snippet.chars().skip(start_char).take(end_char - start_char));
    if end_char < snippet_chars {
        result.push('…');
    }
    result
}

/// Find exact unique match
#[allow(dead_code)]
#[must_use]
pub fn find_exact_unique(content: &str, old_text: &str) -> Option<EditSpec> {
    match find_exact_match(content, old_text)? {
        MatchOutcome::Unique(spec) => Some(spec),
        MatchOutcome::Duplicate(_) => None,
    }
}

/// Find match with different indentation
fn find_dedent_match(content: &str, old_text: &str) -> Option<MatchOutcome> {
    let old_indent = common_leading_whitespace(old_text);
    let mut duplicate = None;

    for line in content.lines() {
        let line_indent = leading_whitespace(line);
        if line_indent != old_indent && !line_indent.is_empty() {
            let adjusted = reindent_text(old_text, &old_indent, line_indent);
            if let Some(outcome) = find_exact_match(content, &adjusted) {
                match outcome {
                    MatchOutcome::Unique(spec) => return Some(MatchOutcome::Unique(spec)),
                    outcome @ MatchOutcome::Duplicate(_) => duplicate = duplicate.or(Some(outcome)),
                }
            }
        }
    }
    duplicate
}

/// Find match with trimmed first/last lines
fn find_trimmed_match(content: &str, old_text: &str) -> Option<MatchOutcome> {
    let lines: Vec<&str> = old_text.lines().collect();
    if lines.len() <= 2 {
        return None;
    }

    let without_first = lines[1..].join("\n");
    let mut duplicate = None;
    if let Some(outcome) = find_exact_match(content, &without_first) {
        match outcome {
            MatchOutcome::Unique(mut spec) => {
                if spec.offset > 0 {
                    #[allow(clippy::string_slice)]
                    let before = &content[..spec.offset];
                    let first_line_with_newline = format!("{}\n", lines[0]);
                    if before.ends_with(&first_line_with_newline) {
                        spec.offset -= first_line_with_newline.len();
                        spec.length += first_line_with_newline.len();
                    }
                }
                return Some(MatchOutcome::Unique(spec));
            }
            outcome @ MatchOutcome::Duplicate(_) => duplicate = Some(outcome),
        }
    }

    let without_last = lines[..lines.len() - 1].join("\n");
    if let Some(outcome) = find_exact_match(content, &without_last) {
        match outcome {
            MatchOutcome::Unique(spec) => return Some(MatchOutcome::Unique(spec)),
            MatchOutcome::Duplicate(_) => return duplicate.or(Some(outcome)),
        }
    }

    duplicate
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
fn find_normalised_match(content: &str, old_text: &str) -> Option<MatchOutcome> {
    let skel_old: String = skeleton(old_text).collect();

    if skel_old == old_text {
        return None;
    }

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
    skel_to_orig.push(content.len());

    let mut matches = skel_content.match_indices(&skel_old);
    let (first_offset, _) = matches.next()?;
    let first_range =
        original_range_from_skeleton(&skel_to_orig, first_offset, skel_old.len(), content.len());
    let Some((second_offset, _)) = matches.next() else {
        return Some(MatchOutcome::Unique(EditSpec {
            offset: first_range.0,
            length: first_range.1,
        }));
    };
    let second_range =
        original_range_from_skeleton(&skel_to_orig, second_offset, skel_old.len(), content.len());

    Some(MatchOutcome::Duplicate(
        duplicate_match_diagnostics_from_ranges(
            content,
            std::iter::once(first_range)
                .chain(std::iter::once(second_range))
                .chain(matches.map(|(offset, _)| {
                    original_range_from_skeleton(
                        &skel_to_orig,
                        offset,
                        skel_old.len(),
                        content.len(),
                    )
                })),
        ),
    ))
}

fn original_range_from_skeleton(
    skel_to_orig: &[usize],
    offset: usize,
    length: usize,
    content_len: usize,
) -> (usize, usize) {
    let orig_start = skel_to_orig[offset];
    let orig_end = if offset + length < skel_to_orig.len() {
        skel_to_orig[offset + length]
    } else {
        content_len
    };
    (orig_start, orig_end - orig_start)
}

/// Get leading whitespace from a string
#[must_use]
pub fn leading_whitespace(s: &str) -> &str {
    let trimmed = s.trim_start();
    // Safety: `s.len() - trimmed.len()` is the byte length of leading whitespace,
    // which is a valid boundary since `trim_start()` splits at a char boundary
    #[allow(clippy::string_slice)]
    &s[..s.len() - trimmed.len()]
}

/// Get common leading whitespace across all non-empty lines
#[must_use]
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
#[must_use]
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
    fn test_find_all_exact() {
        let specs = find_all_exact("foo bar foo baz foo", "foo");
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].offset, 0);
        assert_eq!(specs[1].offset, 8);
        assert_eq!(specs[2].offset, 16);
        assert!(specs.iter().all(|s| s.length == 3));
    }

    #[test]
    fn test_find_all_exact_empty_needle_yields_nothing() {
        assert!(find_all_exact("anything", "").is_empty());
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
        match err {
            PatchError::OldTextNotUnique(diagnostics) => {
                assert_eq!(diagnostics.total, 2);
                assert_eq!(diagnostics.omitted, 0);
                assert_eq!(diagnostics.reported.len(), 2);
                assert_eq!(diagnostics.reported[0].start_line, 1);
                assert_eq!(diagnostics.reported[1].start_line, 1);
                assert_eq!(diagnostics.reported[0].snippet, "hello hello");
            }
            other @ (PatchError::ReplaceOnNonexistent
            | PatchError::MissingOldText
            | PatchError::ClipboardNotFound(_)
            | PatchError::OldTextNotFound
            | PatchError::EditOutOfBounds
            | PatchError::OverlappingEdits
            | PatchError::ReplaceAllInexact
            | PatchError::ReindentPrefixMismatch { .. }
            | PatchError::NoPatches) => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn duplicate_match_diagnostics_include_multiline_snippets() {
        let content = "alpha\nmatch {\n    value\n}\nbeta\nmatch {\n    value\n}\ngamma";
        let diagnostics = duplicate_match_diagnostics(content, "match {\n    value\n}");

        assert_eq!(diagnostics.total, 2);
        assert_eq!(diagnostics.reported[0].start_line, 2);
        assert_eq!(
            diagnostics.reported[0].snippet,
            "alpha\nmatch {\n    value\n}\nbeta"
        );
        assert_eq!(diagnostics.reported[1].start_line, 6);
        assert_eq!(
            diagnostics.reported[1].snippet,
            "beta\nmatch {\n    value\n}\ngamma"
        );
    }

    #[test]
    fn duplicate_match_diagnostics_are_bounded() {
        let content = "x\nx\nx\nx\nx\nx\nx";
        let diagnostics = duplicate_match_diagnostics(content, "x");

        assert_eq!(diagnostics.total, 7);
        assert_eq!(diagnostics.reported.len(), 5);
        assert_eq!(diagnostics.omitted, 2);
        assert_eq!(diagnostics.reported[4].start_line, 5);
    }

    #[test]
    fn duplicate_match_snippets_are_capped_by_character_count() {
        let long_prefix = "a".repeat(1_000);
        let long_suffix = "b".repeat(1_000);
        let content = format!("{long_prefix}TARGET{long_suffix} TARGET");
        let diagnostics = duplicate_match_diagnostics(&content, "TARGET");

        assert_eq!(diagnostics.total, 2);
        assert!(
            diagnostics.reported[0].snippet.chars().count() <= MAX_SNIPPET_CHARS + 2,
            "snippet was not capped: {} chars",
            diagnostics.reported[0].snippet.chars().count()
        );
        assert!(diagnostics.reported[0].snippet.contains("TARGET"));
        assert!(diagnostics.reported[0].snippet.starts_with('…'));
        assert!(diagnostics.reported[0].snippet.ends_with('…'));
    }

    #[test]
    fn fuzzy_dedent_duplicate_reports_locations() {
        let content = "\tindented line\nother\n\tindented line";
        let err = find_unique_match(content, "  indented line").unwrap_err();

        match err {
            PatchError::OldTextNotUnique(diagnostics) => {
                assert_eq!(diagnostics.total, 2);
                assert_eq!(diagnostics.reported[0].start_line, 1);
                assert_eq!(diagnostics.reported[1].start_line, 3);
                assert!(diagnostics.reported[0].snippet.contains("\tindented line"));
            }
            other @ (PatchError::ReplaceOnNonexistent
            | PatchError::MissingOldText
            | PatchError::ClipboardNotFound(_)
            | PatchError::OldTextNotFound
            | PatchError::EditOutOfBounds
            | PatchError::OverlappingEdits
            | PatchError::ReplaceAllInexact
            | PatchError::ReindentPrefixMismatch { .. }
            | PatchError::NoPatches) => {
                panic!("expected fuzzy duplicate diagnostics, got {other:?}")
            }
        }
    }

    #[test]
    fn fuzzy_normalised_duplicate_reports_locations() {
        let content = "say \"hello\"\nagain\nsay \"hello\"";
        let err = find_unique_match(content, "say \u{201C}hello\u{201D}").unwrap_err();

        match err {
            PatchError::OldTextNotUnique(diagnostics) => {
                assert_eq!(diagnostics.total, 2);
                assert_eq!(diagnostics.reported[0].start_line, 1);
                assert_eq!(diagnostics.reported[1].start_line, 3);
                assert!(diagnostics.reported[0].snippet.contains("say \"hello\""));
            }
            other @ (PatchError::ReplaceOnNonexistent
            | PatchError::MissingOldText
            | PatchError::ClipboardNotFound(_)
            | PatchError::OldTextNotFound
            | PatchError::EditOutOfBounds
            | PatchError::OverlappingEdits
            | PatchError::ReplaceAllInexact
            | PatchError::ReindentPrefixMismatch { .. }
            | PatchError::NoPatches) => {
                panic!("expected normalised duplicate diagnostics, got {other:?}")
            }
        }
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
