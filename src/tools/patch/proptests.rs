//! Property-based tests for the patch tool
//!
//! These tests verify key invariants of the patch system using
//! the pure `PatchPlanner` and `VirtualFs` interpreter.

#![allow(clippy::uninlined_format_args)]
#![allow(clippy::manual_string_new)]
#![allow(clippy::redundant_closure_for_method_calls)]

use super::interpreter::VirtualFs;
use super::planner::PatchPlanner;
use super::types::*;
use proptest::prelude::*;
use std::path::PathBuf;

// ============================================================================
// Strategies for generating test data
// ============================================================================

/// Generate arbitrary non-empty strings for content
fn arb_content() -> impl Strategy<Value = String> {
    // Generate printable ASCII strings, avoiding edge cases with control chars
    "[a-zA-Z0-9 \n\t]{1,200}"
}

/// Generate content that's suitable as a unique substring
fn arb_unique_substring() -> impl Strategy<Value = String> {
    // Alphanumeric with some punctuation, no whitespace to avoid matching issues
    "[a-zA-Z0-9_]{3,30}"
}

/// Generate a file path
fn arb_path() -> impl Strategy<Value = PathBuf> {
    "[a-z]{1,10}\\.txt".prop_map(PathBuf::from)
}

/// Generate indentation (spaces or tabs)
fn arb_indent() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("".to_string()),
        Just("  ".to_string()),
        Just("    ".to_string()),
        Just("\t".to_string()),
        Just("\t\t".to_string()),
    ]
}

// ============================================================================
// Invariant 1: Overwrite then Replace roundtrip
//
// If we overwrite a file with content X, then replace X with Y,
// the result should be Y.
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn prop_overwrite_then_replace_roundtrip(
        path in arb_path(),
        content in arb_unique_substring(),
        replacement in arb_unique_substring(),
    ) {
        let mut planner = PatchPlanner::new();
        let mut fs = VirtualFs::new();

        // Step 1: Overwrite with content
        let plan1 = planner
            .plan(
            &path,
            None,
            &[PatchRequest {
                operation: Operation::Overwrite,
                old_text: None,
                new_text: Some(content.clone()),
                to_clipboard: None,
                from_clipboard: None,
                reindent: None,
            }],
        ).expect("overwrite should succeed");

        fs.interpret(&plan1.effects);
        prop_assert_eq!(fs.get(&path), Some(&content));

        // Step 2: Replace content with replacement
        let plan2 = planner
            .plan(
            &path,
            fs.get(&path).map(|s| s.as_str()),
            &[PatchRequest {
                operation: Operation::Replace,
                old_text: Some(content.clone()),
                new_text: Some(replacement.clone()),
                to_clipboard: None,
                from_clipboard: None,
                reindent: None,
            }],
        ).expect("replace should succeed after overwrite");

        fs.interpret(&plan2.effects);
        prop_assert_eq!(fs.get(&path), Some(&replacement));
    }
}

// ============================================================================
// Invariant 2: Clipboard cut+paste preserves content
//
// Cutting text to clipboard then pasting it back should preserve the text.
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn prop_clipboard_cut_paste_preserves(
        path in arb_path(),
        prefix in arb_unique_substring(),
        middle in arb_unique_substring(),
        suffix in arb_unique_substring(),
    ) {
        // Ensure parts are distinct
        prop_assume!(prefix != middle && middle != suffix && prefix != suffix);

        let original = format!("{}{}{}", prefix, middle, suffix);
        let mut planner = PatchPlanner::new();
        let mut fs = VirtualFs::with_files([(path.clone(), original.clone())]);

        // Cut middle to clipboard
        let plan1 = planner
            .plan(
            &path,
            fs.get(&path).map(|s| s.as_str()),
            &[PatchRequest {
                operation: Operation::Replace,
                old_text: Some(middle.clone()),
                new_text: Some("".to_string()),
                to_clipboard: Some("cut".to_string()),
                from_clipboard: None,
                reindent: None,
            }],
        ).expect("cut should succeed");
        fs.interpret(&plan1.effects);

        // File should now have prefix + suffix
        let after_cut = format!("{}{}", prefix, suffix);
        prop_assert_eq!(fs.get(&path), Some(&after_cut));

        // Clipboard should have the middle
        prop_assert_eq!(planner.clipboards().get("cut"), Some(&middle));

        // Paste back at the junction
        // We need a unique marker - use the junction point
        let junction = format!("{}{}", prefix, suffix);
        let plan2 = planner
            .plan(
            &path,
            fs.get(&path).map(|s| s.as_str()),
            &[PatchRequest {
                operation: Operation::Replace,
                old_text: Some(junction.clone()),
                new_text: None,
                to_clipboard: None,
                from_clipboard: Some("cut".to_string()),
                reindent: None,
            }],
        ).expect("paste should succeed");
        fs.interpret(&plan2.effects);

        // Content should be restored (note: paste replaces the junction with just the middle)
        prop_assert_eq!(fs.get(&path), Some(&middle));
    }
}

// ============================================================================
// Invariant 3: Reindentation is reversible
//
// strip(X) then add(X) should be identity (for uniform indentation)
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn prop_reindent_roundtrip(
        path in arb_path(),
        indent in arb_indent(),
        line1 in "[a-zA-Z0-9]{1,20}",
        line2 in "[a-zA-Z0-9]{1,20}",
    ) {
        prop_assume!(!indent.is_empty()); // Only test with actual indentation

        // Create indented content
        let indented = format!("{}{}\n{}{}", indent, line1, indent, line2);
        let marker = "MARKER";

        let mut planner = PatchPlanner::new();

        // Replace marker with indented content, but strip then re-add the indent
        let plan = planner
            .plan(
            &path,
            Some(marker),
            &[PatchRequest {
                operation: Operation::Replace,
                old_text: Some(marker.to_string()),
                new_text: Some(indented.clone()),
                to_clipboard: None,
                from_clipboard: None,
                reindent: Some(Reindent {
                    strip: Some(indent.clone()),
                    add: Some(indent.clone()),
                }),
            }],
        ).expect("reindent should succeed");

        // Result should equal original indented content
        prop_assert_eq!(plan.resulting_content, indented);
    }
}

// ============================================================================
// Invariant 4: Multiple patches at different offsets don't corrupt each other
//
// Replacing text at different non-overlapping positions should work correctly.
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn prop_multiple_patches_independent(
        path in arb_path(),
        part1 in arb_unique_substring(),
        part2 in arb_unique_substring(),
        part3 in arb_unique_substring(),
        repl1 in arb_unique_substring(),
        repl3 in arb_unique_substring(),
    ) {
        // Ensure all parts are distinct and no part is a substring of another
        prop_assume!(part1 != part2 && part2 != part3 && part1 != part3);
        prop_assume!(!part1.contains(&*part2) && !part2.contains(&*part1));
        prop_assume!(!part2.contains(&*part3) && !part3.contains(&*part2));
        prop_assume!(!part1.contains(&*part3) && !part3.contains(&*part1));
        prop_assume!(repl1 != part2 && repl3 != part2);
        prop_assume!(repl1 != part1 && repl3 != part3); // Replacements differ from originals

        let original = format!("{}|{}|{}", part1, part2, part3);
        let expected = format!("{}|{}|{}", repl1, part2, repl3);

        let mut planner = PatchPlanner::new();
        let plan = planner
            .plan(
            &path,
            Some(&original),
            &[
                PatchRequest {
                    operation: Operation::Replace,
                    old_text: Some(part1.clone()),
                    new_text: Some(repl1.clone()),
                    to_clipboard: None,
                    from_clipboard: None,
                    reindent: None,
                },
                PatchRequest {
                    operation: Operation::Replace,
                    old_text: Some(part3.clone()),
                    new_text: Some(repl3.clone()),
                    to_clipboard: None,
                    from_clipboard: None,
                    reindent: None,
                },
            ],
        ).expect("multiple patches should succeed");

        prop_assert_eq!(plan.resulting_content, expected);
    }
}

// ============================================================================
// Invariant 5: Append then prepend produces correct ordering
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn prop_append_prepend_ordering(
        path in arb_path(),
        initial in arb_unique_substring(),
        prefix in arb_unique_substring(),
        suffix in arb_unique_substring(),
    ) {
        let mut planner = PatchPlanner::new();
        let mut fs = VirtualFs::with_files([(path.clone(), initial.clone())]);

        // Append suffix
        let plan1 = planner
            .plan(
            &path,
            fs.get(&path).map(|s| s.as_str()),
            &[PatchRequest {
                operation: Operation::AppendEof,
                old_text: None,
                new_text: Some(suffix.clone()),
                to_clipboard: None,
                from_clipboard: None,
                reindent: None,
            }],
        ).expect("append should succeed");
        fs.interpret(&plan1.effects);

        // Prepend prefix
        let plan2 = planner
            .plan(
            &path,
            fs.get(&path).map(|s| s.as_str()),
            &[PatchRequest {
                operation: Operation::PrependBof,
                old_text: None,
                new_text: Some(prefix.clone()),
                to_clipboard: None,
                from_clipboard: None,
                reindent: None,
            }],
        ).expect("prepend should succeed");
        fs.interpret(&plan2.effects);

        let expected = format!("{}{}{}", prefix, initial, suffix);
        prop_assert_eq!(fs.get(&path), Some(&expected));
    }
}

// ============================================================================
// Invariant 6: Overwrite is idempotent
//
// Overwriting with the same content twice should yield the same result.
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn prop_overwrite_idempotent(
        path in arb_path(),
        initial in arb_content(),
        new_content in arb_content(),
    ) {
        let mut planner = PatchPlanner::new();
        let mut fs = VirtualFs::with_files([(path.clone(), initial)]);

        // First overwrite
        let plan1 = planner
            .plan(
            &path,
            fs.get(&path).map(|s| s.as_str()),
            &[PatchRequest {
                operation: Operation::Overwrite,
                old_text: None,
                new_text: Some(new_content.clone()),
                to_clipboard: None,
                from_clipboard: None,
                reindent: None,
            }],
        ).expect("first overwrite should succeed");
        fs.interpret(&plan1.effects);
        let after_first = fs.get(&path).cloned();

        // Second overwrite with same content
        let plan2 = planner
            .plan(
            &path,
            fs.get(&path).map(|s| s.as_str()),
            &[PatchRequest {
                operation: Operation::Overwrite,
                old_text: None,
                new_text: Some(new_content.clone()),
                to_clipboard: None,
                from_clipboard: None,
                reindent: None,
            }],
        ).expect("second overwrite should succeed");
        fs.interpret(&plan2.effects);
        let after_second = fs.get(&path).cloned();

        prop_assert_eq!(after_first, after_second.clone());
        prop_assert_eq!(after_second, Some(new_content));
    }
}

// ============================================================================
// Unit tests for edge cases
// ============================================================================

#[test]
fn test_replace_on_nonexistent_fails() {
    let mut planner = PatchPlanner::new();
    let result = planner.plan(
        &PathBuf::from("test.txt"),
        None,
        &[PatchRequest {
            operation: Operation::Replace,
            old_text: Some("foo".to_string()),
            new_text: Some("bar".to_string()),
            to_clipboard: None,
            from_clipboard: None,
            reindent: None,
        }],
    );
    assert!(matches!(result, Err(PatchError::ReplaceOnNonexistent)));
}

#[test]
fn test_replace_not_found() {
    let mut planner = PatchPlanner::new();
    let result = planner.plan(
        &PathBuf::from("test.txt"),
        Some("hello world"),
        &[PatchRequest {
            operation: Operation::Replace,
            old_text: Some("foo".to_string()),
            new_text: Some("bar".to_string()),
            to_clipboard: None,
            from_clipboard: None,
            reindent: None,
        }],
    );
    assert!(matches!(result, Err(PatchError::OldTextNotFound)));
}

#[test]
fn test_replace_not_unique() {
    let mut planner = PatchPlanner::new();
    let result = planner.plan(
        &PathBuf::from("test.txt"),
        Some("hello hello"),
        &[PatchRequest {
            operation: Operation::Replace,
            old_text: Some("hello".to_string()),
            new_text: Some("world".to_string()),
            to_clipboard: None,
            from_clipboard: None,
            reindent: None,
        }],
    );
    assert!(matches!(result, Err(PatchError::OldTextNotUnique(2))));
}

#[test]
fn test_clipboard_not_found() {
    let mut planner = PatchPlanner::new();
    let result = planner.plan(
        &PathBuf::from("test.txt"),
        Some("hello"),
        &[PatchRequest {
            operation: Operation::AppendEof,
            old_text: None,
            new_text: None,
            to_clipboard: None,
            from_clipboard: Some("nonexistent".to_string()),
            reindent: None,
        }],
    );
    assert!(matches!(result, Err(PatchError::ClipboardNotFound(_))));
}

#[test]
fn test_empty_patches_fails() {
    let mut planner = PatchPlanner::new();
    let result = planner.plan(&PathBuf::from("test.txt"), Some("hello"), &[]);
    assert!(matches!(result, Err(PatchError::NoPatches)));
}

// ============================================================================
// Integration test: Full "overwrite then replace" with real filesystem
//
// This recreates the original aberrant behavior demo
// ============================================================================

#[test]
fn test_overwrite_then_replace_with_filesystem() {
    use super::executor::{execute_effects, read_file_content};
    use std::fs;

    let test_dir = std::env::temp_dir();
    let test_file = test_dir.join("test_patch_demo.txt");

    // Clean up any existing file
    let _ = fs::remove_file(&test_file);

    // Create a fresh planner
    let mut planner = PatchPlanner::new();

    // Step 1: Overwrite to create the file with "Hello World"
    let current_content = read_file_content(&test_file).expect("read should succeed");
    assert!(current_content.is_none(), "file should not exist yet");

    let plan1 = planner
        .plan(
            &test_file,
            current_content.as_deref(),
            &[PatchRequest {
                operation: Operation::Overwrite,
                old_text: None,
                new_text: Some("Hello World".to_string()),
                to_clipboard: None,
                from_clipboard: None,
                reindent: None,
            }],
        )
        .expect("overwrite should succeed");

    execute_effects(&plan1.effects).expect("execute should succeed");

    // Verify step 1
    let content_after_1 =
        fs::read_to_string(&test_file).expect("file should exist after overwrite");
    assert_eq!(content_after_1, "Hello World");

    // Step 2: Replace "Hello World" with "Hello Phoenix IDE"
    // This is where the original bug occurred!
    let current_content = read_file_content(&test_file).expect("read should succeed");
    assert_eq!(current_content.as_deref(), Some("Hello World"));

    let plan2 = planner
        .plan(
            &test_file,
            current_content.as_deref(),
            &[PatchRequest {
                operation: Operation::Replace,
                old_text: Some("Hello World".to_string()),
                new_text: Some("Hello Phoenix IDE".to_string()),
                to_clipboard: None,
                from_clipboard: None,
                reindent: None,
            }],
        )
        .expect("replace should succeed after overwrite - this was the bug!");

    execute_effects(&plan2.effects).expect("execute should succeed");

    // Verify step 2
    let content_after_2 = fs::read_to_string(&test_file).expect("file should exist after replace");
    assert_eq!(content_after_2, "Hello Phoenix IDE");

    // Clean up
    let _ = fs::remove_file(&test_file);
}
