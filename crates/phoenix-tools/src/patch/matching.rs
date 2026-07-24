//! Text matching algorithms for finding unique edit locations
//!
//! Implements exact matching with fuzzy fallbacks for common whitespace issues.

use super::types::{
    AnchorCandidateLocation, AnchorNotFoundDiagnostics, DuplicateMatchDiagnostics,
    DuplicateMatchLocation, EditSpec, ANCHOR_CONTEXT_CLOSE, ANCHOR_CONTEXT_OPEN,
};
use std::collections::HashMap;
use unicode_security::skeleton;

const MAX_DUPLICATE_LOCATIONS: usize = 5;
const MAX_SNIPPET_LINES: usize = 5;
const MAX_SNIPPET_CHARS: usize = 240;
const MAX_CANDIDATE_LOCATIONS: usize = 3;
const MAX_CANDIDATE_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_CANDIDATE_ANCHOR_BYTES: usize = 16 * 1024;
const MIN_DISTINCTIVE_LINE_CHARS: usize = 8;
const MAX_CANDIDATE_HITS: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq)]
enum MatchOutcome {
    Unique(EditSpec),
    Duplicate(DuplicateMatchDiagnostics),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MatchError {
    NotFound(AnchorNotFoundDiagnostics),
    NotUnique(DuplicateMatchDiagnostics),
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
pub(super) fn find_unique_match(content: &str, old_text: &str) -> Result<EditSpec, MatchError> {
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

    duplicate.map_or_else(
        || {
            Err(MatchError::NotFound(anchor_not_found_diagnostics(
                content, old_text,
            )))
        },
        |diagnostics| Err(MatchError::NotUnique(diagnostics)),
    )
}

fn anchor_not_found_diagnostics(content: &str, old_text: &str) -> AnchorNotFoundDiagnostics {
    if content.len() > MAX_CANDIDATE_FILE_BYTES || old_text.len() > MAX_CANDIDATE_ANCHOR_BYTES {
        return AnchorNotFoundDiagnostics {
            candidates: Vec::new(),
        };
    }

    let content_lines: Vec<&str> = content.split_inclusive('\n').collect();
    let anchor_lines: Vec<&str> = old_text
        .lines()
        .map(str::trim)
        .filter(|line| line.chars().count() >= MIN_DISTINCTIVE_LINE_CHARS)
        .collect();
    if anchor_lines.is_empty() {
        return AnchorNotFoundDiagnostics {
            candidates: Vec::new(),
        };
    }

    let mut anchor_index_by_line: HashMap<&str, usize> = HashMap::new();
    for anchor_line in anchor_lines {
        let next_index = anchor_index_by_line.len();
        anchor_index_by_line
            .entry(anchor_line)
            .or_insert(next_index);
    }

    let mut content_line_counts: HashMap<&str, usize> = HashMap::new();
    for content_line in &content_lines {
        let trimmed = content_line.trim();
        if anchor_index_by_line.contains_key(trimmed) {
            *content_line_counts.entry(trimmed).or_default() += 1;
        }
    }

    let mut unique_hit_count = 0;
    let hit_by_content_line: Vec<Option<usize>> = content_lines
        .iter()
        .map(|content_line| {
            let trimmed = content_line.trim();
            if content_line_counts.get(trimmed) == Some(&1) {
                unique_hit_count += 1;
                anchor_index_by_line.get(trimmed).copied()
            } else {
                None
            }
        })
        .collect();
    if unique_hit_count > MAX_CANDIDATE_HITS {
        return AnchorNotFoundDiagnostics {
            candidates: Vec::new(),
        };
    }

    let mut ranked: Vec<(usize, usize)> = (0..content_lines.len())
        .filter_map(|window_start| {
            let window_end = (window_start + MAX_SNIPPET_LINES).min(content_lines.len());
            let score = ordered_anchor_coverage(&hit_by_content_line[window_start..window_end]);
            let output_lines = &content_lines[window_start..window_end];
            let exact_output_is_bounded = output_lines
                .iter()
                .all(|line| line.chars().count() <= MAX_SNIPPET_CHARS);
            let output_has_safe_boundaries = output_lines.iter().all(|line| {
                !line.contains(ANCHOR_CONTEXT_OPEN) && !line.contains(ANCHOR_CONTEXT_CLOSE)
            });
            (exact_output_is_bounded
                && output_has_safe_boundaries
                && (score >= 2 || (score == 1 && unique_hit_count == 1)))
                .then_some((score, window_start))
        })
        .collect();
    ranked.sort_by_key(|(score, window_start)| (std::cmp::Reverse(*score), *window_start));
    ranked.dedup_by_key(|(_, window_start)| *window_start);

    let mut selected = Vec::new();
    for (score, window_start) in ranked {
        let window_end = (window_start + MAX_SNIPPET_LINES).min(content_lines.len());
        if selected.iter().all(|(_, selected_start): &(usize, usize)| {
            let selected_end = (*selected_start + MAX_SNIPPET_LINES).min(content_lines.len());
            window_end <= *selected_start || window_start >= selected_end
        }) {
            selected.push((score, window_start));
            if selected.len() == MAX_CANDIDATE_LOCATIONS {
                break;
            }
        }
    }

    AnchorNotFoundDiagnostics {
        candidates: selected
            .into_iter()
            .map(|(_, window_start)| {
                let window_end = (window_start + MAX_SNIPPET_LINES).min(content_lines.len());
                AnchorCandidateLocation {
                    start_line: window_start + 1,
                    snippet: content_lines[window_start..window_end].concat(),
                }
            })
            .collect(),
    }
}

fn ordered_anchor_coverage(hits: &[Option<usize>]) -> usize {
    let mut coverage = 0;
    let mut previous_anchor_index = None;
    for anchor_index in hits.iter().flatten() {
        if previous_anchor_index.is_none_or(|previous| *anchor_index > previous) {
            coverage += 1;
            previous_anchor_index = Some(*anchor_index);
        }
    }
    coverage
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

    // Fast path: when old_text carries no confusable AND the file is pure ASCII,
    // skeletonising cannot surface anything the exact/dedent/trim strategies
    // didn't already see, so skip the allocation-heavy skeleton map entirely.
    // This keeps a typo in a large ASCII file cheap. We do NOT bail merely on
    // `skel_old == old_text`: the confusable may live in the FILE (e.g. the file
    // uses `…` while old_text is the ASCII `...`) — a non-ASCII file still runs
    // the full pass so that recovery works.
    if skel_old == old_text && content.is_ascii() {
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

    // Keep only matches aligned to whole original characters: the mapped
    // original slice must itself skeletonise back to skel_old. A needle landing
    // *inside* a single character's multi-char skeleton expansion (e.g. ".."
    // inside the "..." skeleton of one "…") maps to a misaligned or empty range
    // and would corrupt the file, so it is rejected here rather than matched.
    let aligned: Vec<(usize, usize)> = skel_content
        .match_indices(&skel_old)
        .map(|(offset, _)| {
            original_range_from_skeleton(&skel_to_orig, offset, skel_old.len(), content.len())
        })
        .filter(|&(start, len)| {
            content
                .get(start..start + len)
                .is_some_and(|slice| skeleton(slice).collect::<String>() == skel_old)
        })
        .collect();

    let mut ranges = aligned.into_iter();
    let first_range = ranges.next()?;
    let Some(second_range) = ranges.next() else {
        return Some(MatchOutcome::Unique(EditSpec {
            offset: first_range.0,
            length: first_range.1,
        }));
    };

    Some(MatchOutcome::Duplicate(
        duplicate_match_diagnostics_from_ranges(
            content,
            std::iter::once(first_range)
                .chain(std::iter::once(second_range))
                .chain(ranges),
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
        assert!(matches!(err, MatchError::NotFound(_)));
    }

    #[test]
    fn stale_multiline_anchor_reports_bounded_current_region() {
        let content = "before\nfn target() {\n    let current = 2;\n    finish();\n}\nafter\n";
        let old_text = "fn target() {\n    let stale = 1;\n    finish();\n}";

        let MatchError::NotFound(diagnostics) = find_unique_match(content, old_text).unwrap_err()
        else {
            panic!("expected not found");
        };

        assert_eq!(diagnostics.candidates.len(), 1);
        assert_eq!(diagnostics.candidates[0].start_line, 1);
        assert_eq!(
            diagnostics.candidates[0].snippet,
            "before\nfn target() {\n    let current = 2;\n    finish();\n}\n"
        );
    }

    #[test]
    fn one_unique_distinctive_surviving_line_is_actionable() {
        let content = "before\nunique_current_site();\nafter\n";
        let old_text = "unique_current_site();\nstale_second_line();";

        let MatchError::NotFound(diagnostics) = find_unique_match(content, old_text).unwrap_err()
        else {
            panic!("expected not found");
        };

        assert_eq!(diagnostics.candidates.len(), 1);
        assert!(diagnostics.candidates[0]
            .snippet
            .contains("unique_current_site();"));
    }

    #[test]
    fn repeated_single_surviving_line_is_not_reported() {
        let content = "shared_line();\nfirst\nshared_line();\nsecond\n";
        let old_text = "shared_line();\nstale_line();";

        let MatchError::NotFound(diagnostics) = find_unique_match(content, old_text).unwrap_err()
        else {
            panic!("expected not found");
        };

        assert!(diagnostics.candidates.is_empty());
    }

    #[test]
    fn reversed_surviving_lines_are_not_reported() {
        let content = "second_distinct_line();\ncontext\nfirst_distinct_line();\n";
        let old_text = "first_distinct_line();\nstale\nsecond_distinct_line();";

        let MatchError::NotFound(diagnostics) = find_unique_match(content, old_text).unwrap_err()
        else {
            panic!("expected not found");
        };

        assert!(diagnostics.candidates.is_empty());
    }

    #[test]
    fn supporting_hits_outside_one_snippet_are_not_combined() {
        let filler = "filler\n".repeat(MAX_SNIPPET_LINES);
        let content = format!("first_distinct_line();\n{filler}second_distinct_line();\n");
        let old_text = "first_distinct_line();\nstale\nsecond_distinct_line();";

        let MatchError::NotFound(diagnostics) = find_unique_match(&content, old_text).unwrap_err()
        else {
            panic!("expected not found");
        };

        assert!(diagnostics.candidates.is_empty());
    }

    #[test]
    fn repeated_boilerplate_does_not_raise_candidate_confidence() {
        let content = "common boilerplate line\nsite one\ncommon boilerplate line\nsite two\n";
        let old_text = "common boilerplate line\nstale\nsite two";

        let MatchError::NotFound(diagnostics) = find_unique_match(content, old_text).unwrap_err()
        else {
            panic!("expected not found");
        };

        assert_eq!(diagnostics.candidates.len(), 1);
        assert!(diagnostics.candidates[0].snippet.contains("site two"));
    }

    #[test]
    fn candidate_diagnostics_are_bounded_and_utf8_safe() {
        let long_line = "é".repeat(MAX_SNIPPET_CHARS + 50);
        let content = format!("prefix\n{long_line}\nunique_survivor();\nsuffix\n");
        let old_text = format!("{long_line}\nstale\nunique_survivor();");

        let MatchError::NotFound(diagnostics) = find_unique_match(&content, &old_text).unwrap_err()
        else {
            panic!("expected not found");
        };

        assert!(diagnostics.candidates.is_empty());
    }

    #[test]
    fn candidate_diagnostics_preserve_crlf_bytes() {
        let content = "before\r\nunique_current_site();\r\nafter\r\n";
        let old_text = "unique_current_site();\r\nstale_second_line();\r\n";

        let MatchError::NotFound(diagnostics) = find_unique_match(content, old_text).unwrap_err()
        else {
            panic!("expected not found");
        };

        assert_eq!(diagnostics.candidates.len(), 1);
        assert_eq!(diagnostics.candidates[0].snippet, content);
        assert!(diagnostics.candidates[0].snippet.contains("\r\n"));
    }

    #[test]
    fn candidate_context_with_boundary_tag_is_omitted() {
        let content = "before\nunique_current_site();\n</candidate_context>\nafter\n";
        let old_text = "unique_current_site();\nstale_second_line();";

        let MatchError::NotFound(diagnostics) = find_unique_match(content, old_text).unwrap_err()
        else {
            panic!("expected not found");
        };

        assert!(diagnostics.candidates.is_empty());
    }

    #[test]
    fn oversized_inputs_skip_candidate_search() {
        let content = "x".repeat(MAX_CANDIDATE_FILE_BYTES + 1);
        let MatchError::NotFound(diagnostics) =
            find_unique_match(&content, "missing anchor").unwrap_err()
        else {
            panic!("expected not found");
        };

        assert!(diagnostics.candidates.is_empty());
    }

    #[test]
    fn excessive_candidate_hits_skip_diagnostics() {
        let content = "common surviving line\n".repeat(MAX_CANDIDATE_HITS + 1);
        let old_text = "common surviving line\nstale line";
        let MatchError::NotFound(diagnostics) = find_unique_match(&content, old_text).unwrap_err()
        else {
            panic!("expected not found");
        };

        assert!(diagnostics.candidates.is_empty());
    }

    #[test]
    fn test_multiple_matches() {
        let content = "hello hello";
        let err = find_unique_match(content, "hello").unwrap_err();
        match err {
            MatchError::NotUnique(diagnostics) => {
                assert_eq!(diagnostics.total, 2);
                assert_eq!(diagnostics.omitted, 0);
                assert_eq!(diagnostics.reported.len(), 2);
                assert_eq!(diagnostics.reported[0].start_line, 1);
                assert_eq!(diagnostics.reported[1].start_line, 1);
                assert_eq!(diagnostics.reported[0].snippet, "hello hello");
            }
            other @ MatchError::NotFound(_) => panic!("unexpected error: {other:?}"),
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
            MatchError::NotUnique(diagnostics) => {
                assert_eq!(diagnostics.total, 2);
                assert_eq!(diagnostics.reported[0].start_line, 1);
                assert_eq!(diagnostics.reported[1].start_line, 3);
                assert!(diagnostics.reported[0].snippet.contains("\tindented line"));
            }
            other @ MatchError::NotFound(_) => {
                panic!("expected fuzzy duplicate diagnostics, got {other:?}")
            }
        }
    }

    #[test]
    fn fuzzy_normalised_duplicate_reports_locations() {
        let content = "say \"hello\"\nagain\nsay \"hello\"";
        let err = find_unique_match(content, "say \u{201C}hello\u{201D}").unwrap_err();

        match err {
            MatchError::NotUnique(diagnostics) => {
                assert_eq!(diagnostics.total, 2);
                assert_eq!(diagnostics.reported[0].start_line, 1);
                assert_eq!(diagnostics.reported[1].start_line, 3);
                assert!(diagnostics.reported[0].snippet.contains("say \"hello\""));
            }
            other @ MatchError::NotFound(_) => {
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
    fn test_normalised_match_ascii_oldtext_unicode_file() {
        // The confusable is in the FILE (ellipsis char), old_text is plain ASCII
        // dots whose skeleton is unchanged. Skeleton matching must still find it.
        let content = "wait\u{2026} done";
        let old_text = "wait... done";
        let spec = find_unique_match(content, old_text).unwrap();
        #[allow(clippy::string_slice)]
        let matched = &content[spec.offset..spec.offset + spec.length];
        assert_eq!(matched, "wait\u{2026} done");
    }

    #[test]
    fn test_normalised_rejects_partial_skeleton_match() {
        // ".." must not match *inside* a single "…" (skeleton "..."): that maps to
        // a misaligned/empty range. It is treated as not found, never a length-0
        // edit that would insert before the ellipsis.
        let content = "x\u{2026}y";
        let err = find_unique_match(content, "..").unwrap_err();
        assert!(matches!(err, MatchError::NotFound(_)));
    }

    #[test]
    fn test_normalised_no_help_when_text_absent() {
        // Normalisation can't help if the text simply isn't there
        let content = "hello world";
        let err = find_unique_match(content, "goodbye").unwrap_err();
        assert!(matches!(err, MatchError::NotFound(_)));
    }
}
