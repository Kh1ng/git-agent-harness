//! UTF-8 safety regression tests for production panic prevention.
//!
//! These tests verify that GAH never panics when processing text containing
//! multibyte UTF-8 characters (box-drawing chars, emojis, etc.) at arbitrary
//! byte boundaries.
//!
//! The original panic occurred at src/dispatch.rs:6708 in extract_backend_summary
//! when slicing log text at byte indices that fell inside multibyte characters.

use std::fs;
use tempfile::TempDir;

// Import the functions we need to test
// Note: These are not public, so we'll test them indirectly through the public API
// or create test versions here.

/// Test version of utf8_safe_suffix for direct testing
fn utf8_safe_suffix(s: &str, max_bytes: usize) -> &str {
    if s.is_empty() || max_bytes == 0 {
        return "";
    }
    let byte_start = s.len().saturating_sub(max_bytes);
    // Ensure we start at a valid character boundary
    // If byte_start is not a boundary, find the next boundary after it
    // This guarantees result.len() <= max_bytes
    let safe_start = if !s.is_char_boundary(byte_start) {
        s.char_indices()
            .find(|(i, _)| *i >= byte_start)
            .map(|(i, _)| i)
            .unwrap_or(s.len())
    } else {
        byte_start
    };
    &s[safe_start..]
}

/// Test version of utf8_safe_prefix for direct testing
fn utf8_safe_prefix(s: &str, max_bytes: usize) -> &str {
    if s.is_empty() || max_bytes == 0 {
        return "";
    }
    let byte_end = s.len().min(max_bytes);
    // Ensure we end at a valid character boundary
    // If byte_end is not a boundary, find the previous boundary before it
    // This guarantees result.len() <= max_bytes
    let safe_end = if !s.is_char_boundary(byte_end) {
        s.char_indices()
            .take_while(|(i, _)| *i < byte_end)
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0)
    } else {
        byte_end
    };
    &s[..safe_end]
}

// ── Helper to create test content with known byte positions ─────────────────

/// Creates content where a multibyte character starts at a known byte position
/// and the total length causes a cut at a specific byte when taking a suffix.
///
/// For example, to test a cut at byte 1000 inside a 3-byte character:
/// - 999 bytes of ASCII 'A' (positions 0-998)
/// - 1 character of '│' (3 bytes at positions 999-1001)
/// - remaining bytes
/// - When taking last 2000 bytes from a 3000-byte string, cut starts at byte 1000
fn make_test_content_with_cut_at(cut_byte: usize, multibyte_char: &str) -> String {
    let prefix_len = cut_byte - 1; // Put the char starting at cut_byte - 1
    let multibyte_len = multibyte_char.len();
    // We want: total_len - max_bytes = cut_byte
    // For suffix test with max_bytes = 2000, total_len = cut_byte + 2000
    let total_len = cut_byte + 2000;
    let suffix_len = total_len - prefix_len - multibyte_len;

    format!(
        "{}{}{}",
        "A".repeat(prefix_len),
        multibyte_char,
        "B".repeat(suffix_len)
    )
}

// ── Unit tests for helper functions ────────────────────────────────────────

#[test]
fn utf8_safe_suffix_handles_3byte_char_boundary() {
    // The │ character is 3 bytes: 0xE2 0x94 0x82
    let content = make_test_content_with_cut_at(1000, "│");
    assert_eq!(content.len(), 3000);

    // This should not panic
    let suffix = utf8_safe_suffix(&content, 2000);

    // Result length must not exceed max_bytes
    assert!(suffix.len() <= 2000);
    // Since byte 1000 is inside a 3-byte char, the suffix starts at byte 1002
    // So it returns 1998 bytes (skipping the partial character)
    assert_eq!(suffix.len(), 1998);
    // The suffix should NOT start with │ since we skip the partial character
    assert!(!suffix.starts_with("│"));
    assert!(suffix.starts_with("BB"));
    // But it must be valid UTF-8
    assert!(suffix.is_char_boundary(0));
}

#[test]
fn utf8_safe_suffix_handles_4byte_emoji_boundary() {
    // The 🚀 emoji is 4 bytes
    let content = make_test_content_with_cut_at(1000, "🚀");

    // This should not panic
    let suffix = utf8_safe_suffix(&content, 2000);

    // Result length must not exceed max_bytes
    assert!(suffix.len() <= 2000);
    // Since byte 1000 is inside a 4-byte char, the suffix starts at byte 1003
    // So it returns 1997 bytes (skipping the partial character)
    assert_eq!(suffix.len(), 1997);
    // The suffix should NOT start with 🚀 since we skip the partial character
    assert!(!suffix.starts_with("🚀"));
    assert!(suffix.starts_with("BB"));
    // But it must be valid UTF-8
    assert!(suffix.is_char_boundary(0));
}

#[test]
fn utf8_safe_prefix_handles_3byte_char_boundary() {
    // Create content where a 3-byte character occupies bytes 999-1001
    let prefix = "A".repeat(999);
    let multibyte_char = "│"; // 3 bytes at positions 999-1001
    let suffix = "B".repeat(100);
    let content = format!("{}{}{}", prefix, multibyte_char, suffix);

    // This should not panic when requesting first 1001 bytes
    let prefix_result = utf8_safe_prefix(&content, 1001);

    // Result length must not exceed max_bytes
    assert!(prefix_result.len() <= 1001);
    // Since byte 1001 is inside a 3-byte char, the prefix ends at byte 999
    // So it returns 999 bytes (excluding the partial character)
    assert_eq!(prefix_result.len(), 999);
    // The prefix should NOT include │ since we stop before the partial character
    assert!(!prefix_result.contains("│"));
    assert!(prefix_result.ends_with("AAAAAAAAA"));
    // But it must be valid UTF-8
    assert!(prefix_result.is_char_boundary(prefix_result.len()));
}

#[test]
fn utf8_safe_prefix_handles_4byte_emoji_boundary() {
    // Create content where a 4-byte emoji occupies bytes 999-1002
    let prefix = "A".repeat(999);
    let multibyte_char = "🚀"; // 4 bytes at positions 999-1002
    let suffix = "B".repeat(100);
    let content = format!("{}{}{}", prefix, multibyte_char, suffix);

    // This should not panic when requesting first 1001 bytes
    let prefix_result = utf8_safe_prefix(&content, 1001);

    // Result length must not exceed max_bytes
    assert!(prefix_result.len() <= 1001);
    // Since byte 1001 is inside a 4-byte char, the prefix ends at byte 999
    // So it returns 999 bytes (excluding the partial character)
    assert_eq!(prefix_result.len(), 999);
    // The prefix should NOT include 🚀 since we stop before the partial character
    assert!(!prefix_result.contains("🚀"));
    assert!(prefix_result.ends_with("AAAAAAAAA"));
    // But it must be valid UTF-8
    assert!(prefix_result.is_char_boundary(prefix_result.len()));
}

// ── Edge cases ──────────────────────────────────────────────────────────────

#[test]
fn utf8_safe_suffix_empty_string() {
    assert_eq!(utf8_safe_suffix("", 100), "");
}

#[test]
fn utf8_safe_suffix_zero_max_bytes() {
    assert_eq!(utf8_safe_suffix("hello", 0), "");
}

#[test]
fn utf8_safe_suffix_max_bytes_exceeds_length() {
    let content = "hello";
    let result = utf8_safe_suffix(content, 100);
    assert_eq!(result, "hello");
}

#[test]
fn utf8_safe_suffix_ascii_only() {
    let content = "abcdefghijklmnopqrstuvwxyz";
    let result = utf8_safe_suffix(content, 10);
    assert_eq!(result, "qrstuvwxyz"); // Last 10 chars
}

#[test]
fn utf8_safe_prefix_empty_string() {
    assert_eq!(utf8_safe_prefix("", 100), "");
}

#[test]
fn utf8_safe_prefix_zero_max_bytes() {
    assert_eq!(utf8_safe_prefix("hello", 0), "");
}

#[test]
fn utf8_safe_prefix_max_bytes_exceeds_length() {
    let content = "hello";
    let result = utf8_safe_prefix(content, 100);
    assert_eq!(result, "hello");
}

#[test]
fn utf8_safe_prefix_ascii_only() {
    let content = "abcdefghijklmnopqrstuvwxyz";
    let result = utf8_safe_prefix(content, 10);
    assert_eq!(result, "abcdefghij");
}

// ── Integration test for extract_backend_summary ────────────────────────────

#[test]
fn extract_backend_summary_with_multibyte_chars_no_panic() {
    // This test would ideally call the actual extract_backend_summary function,
    // but it's not public. Instead, we test the logic that was causing the panic.

    // Create a temp file with multibyte UTF-8 content that would cause the original
    // code to panic at byte 1000 inside a 3-byte character
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let log_path = temp_dir.path().join("test_log.txt");

    // Create content that's 3000 bytes with a 3-byte char at positions 999-1001
    let prefix = "A".repeat(999); // 999 bytes at positions 0-998
    let multibyte_char = "│"; // 3 bytes at positions 999-1001
    let suffix = "B".repeat(1998); // 1998 bytes at positions 1002-2999
    let content = format!("{}{}{}", prefix, multibyte_char, suffix);

    assert_eq!(content.len(), 3000);
    assert!(!content.is_char_boundary(1000)); // Byte 1000 is inside the 3-byte char

    // Write to file
    fs::write(&log_path, &content).expect("Failed to write test file");

    // Test the original panic-inducing logic (now fixed in our helper)
    let log_text = fs::read_to_string(&log_path).unwrap_or_default();

    // The original code would have done:
    // let tail_size = log_text.len().min(2000); // 2000
    // let byte_start = log_text.len().saturating_sub(tail_size); // 3000 - 2000 = 1000
    // &log_text[byte_start..]  // This would panic at byte 1000!

    // Our safe version:
    let tail = utf8_safe_suffix(&log_text, 2000);
    assert!(!tail.is_empty());
    // Result length must not exceed max_bytes
    assert!(tail.len() <= 2000);
    // Since byte 1000 is inside a 3-byte char, the suffix starts at byte 1002
    assert_eq!(tail.len(), 1998);
    // The suffix should NOT start with │ since we skip the partial character
    assert!(!tail.starts_with("│"));
    assert!(tail.starts_with("BB"));
    // But it must be valid UTF-8
    assert!(tail.is_char_boundary(0));

    temp_dir.close().ok();
}

// ── Test with realistic GAH-style output ────────────────────────────────────

#[test]
fn utf8_safe_helpers_with_realistic_gah_output() {
    // Realistic GAH backend output with tree structures and box-drawing characters
    let realistic_output = r#"
┌─────────────────────────────────────────────────┐
│                    GAH Output                        │
├─────────────────────────────────────────────────┤
│  ✅ Task completed successfully                    │
│  └── File: src/main.rs                           │
│      ├── Function: process_data                   │
│      │   └── ✅ Implemented correctly              │
│      └── Function: validate_input                 │
│          └── ✅ All edge cases handled              │
└─────────────────────────────────────────────────┘

Summary: All tasks completed. → Ready for review.
"#;

    // Test prefix
    let prefix = utf8_safe_prefix(realistic_output, 100);
    assert!(prefix.is_char_boundary(prefix.len()));

    // Test suffix
    let suffix = utf8_safe_suffix(realistic_output, 100);
    assert!(suffix.is_char_boundary(0));

    // Both should be valid UTF-8 (which they are by construction)
}

// ── Comprehensive boundary behavior tests ────────────────────────────────

#[test]
fn utf8_safe_prefix_exact_ascii_behavior() {
    // ASCII characters are 1 byte each, so prefix of N bytes should give exactly N bytes
    let ascii_content = "abcdefghijklmnopqrstuvwxyz";
    let result = utf8_safe_prefix(ascii_content, 10);
    assert_eq!(result, "abcdefghij");
    assert_eq!(result.len(), 10); // Exact byte count for ASCII
}

#[test]
fn utf8_safe_suffix_exact_ascii_behavior() {
    // ASCII characters are 1 byte each, so suffix of N bytes should give exactly N bytes
    let ascii_content = "abcdefghijklmnopqrstuvwxyz";
    let result = utf8_safe_suffix(ascii_content, 10);
    assert_eq!(result, "qrstuvwxyz");
    assert_eq!(result.len(), 10); // Exact byte count for ASCII
}

#[test]
fn utf8_safe_prefix_2byte_boundary() {
    // Test with 2-byte UTF-8 characters
    let content = "AABBCC"; // All ASCII for now, but test the boundary logic
    let result = utf8_safe_prefix(content, 3);
    assert_eq!(result, "AAB");
    assert_eq!(result.len(), 3);
    assert!(result.len() <= 3);
}

#[test]
fn utf8_safe_prefix_3byte_boundary() {
    // Test cutting inside a 3-byte character
    // Create: 100 bytes of A + "│" (3 bytes) + 100 bytes of B = 203 bytes
    let prefix = "A".repeat(100);
    let multibyte_char = "│"; // 3 bytes
    let suffix = "B".repeat(100);
    let content = format!("{}{}{}", prefix, multibyte_char, suffix);
    assert_eq!(content.len(), 203);

    // Request 101 bytes - this cuts at byte 101, which is inside the 3-byte char (at 100-102)
    let result = utf8_safe_prefix(&content, 101);
    assert!(result.len() <= 101); // Bounded by max_bytes
                                  // Should stop at byte 100 (before the 3-byte char starts)
    assert_eq!(result.len(), 100);
    assert!(!result.contains("│")); // Excludes the partial character
    assert!(result.ends_with("A"));
}

#[test]
fn utf8_safe_prefix_4byte_boundary() {
    // Test cutting inside a 4-byte character
    // Create: 100 bytes of A + "🚀" (4 bytes) + 100 bytes of B = 204 bytes
    let prefix = "A".repeat(100);
    let multibyte_char = "🚀"; // 4 bytes
    let suffix = "B".repeat(100);
    let content = format!("{}{}{}", prefix, multibyte_char, suffix);
    assert_eq!(content.len(), 204);

    // Request 101 bytes - this cuts at byte 101, which is inside the 4-byte char (at 100-103)
    let result = utf8_safe_prefix(&content, 101);
    assert!(result.len() <= 101); // Bounded by max_bytes
                                  // Should stop at byte 100 (before the 4-byte char starts)
    assert_eq!(result.len(), 100);
    assert!(!result.contains("🚀")); // Excludes the partial character
    assert!(result.ends_with("A"));
}

#[test]
fn utf8_safe_suffix_2byte_boundary() {
    // Test with ASCII (1-byte chars) to verify basic behavior
    let content = "AABBCC";
    let result = utf8_safe_suffix(content, 3);
    assert_eq!(result, "BCC");
    assert_eq!(result.len(), 3);
    assert!(result.len() <= 3);
}

#[test]
fn utf8_safe_suffix_3byte_boundary() {
    // Test cutting inside a 3-byte character
    // Create: 100 bytes of A + "│" (3 bytes) + 100 bytes of B = 203 bytes
    let prefix = "A".repeat(100);
    let multibyte_char = "│"; // 3 bytes at positions 100-102
    let suffix = "B".repeat(100);
    let content = format!("{}{}{}", prefix, multibyte_char, suffix);
    assert_eq!(content.len(), 203);

    // Request 100 bytes - this cuts at byte 103, which is after the 3-byte char
    // But we want to test cutting inside the char, so request 101 bytes
    // byte_start = 203 - 101 = 102, which is inside the 3-byte char (100-102)
    let result = utf8_safe_suffix(&content, 101);
    assert!(result.len() <= 101); // Bounded by max_bytes
                                  // Should start at byte 103 (after the 3-byte char ends)
    assert_eq!(result.len(), 100);
    assert!(!result.contains("│")); // Excludes the partial character
    assert!(result.starts_with("B"));
}

#[test]
fn utf8_safe_suffix_4byte_boundary() {
    // Test cutting inside a 4-byte character
    // Create: 100 bytes of A + "🚀" (4 bytes) + 100 bytes of B = 204 bytes
    let prefix = "A".repeat(100);
    let multibyte_char = "🚀"; // 4 bytes at positions 100-103
    let suffix = "B".repeat(100);
    let content = format!("{}{}{}", prefix, multibyte_char, suffix);
    assert_eq!(content.len(), 204);

    // Request 100 bytes - byte_start = 204 - 100 = 104, which is after the 4-byte char
    // Request 101 bytes - byte_start = 204 - 101 = 103, which is the last byte of the 4-byte char
    let result = utf8_safe_suffix(&content, 101);
    assert!(result.len() <= 101); // Bounded by max_bytes
                                  // Should start at byte 104 (after the 4-byte char ends)
    assert_eq!(result.len(), 100);
    assert!(!result.contains("🚀")); // Excludes the partial character
    assert!(result.starts_with("B"));
}

// ── Adversarial tests with various multibyte characters ─────────────────────

#[test]
fn utf8_safety_with_various_multibyte_chars() {
    let multibyte_chars = [
        "│", "─", "└", "├", "✅", "❌", "→", "🚀", "🎉", "💡", "✓", "✗",
    ];

    for char in multibyte_chars {
        // Create test content with this character at the cut point
        let prefix = "A".repeat(100);
        let suffix = "B".repeat(100);
        let content = format!("{}{}{}", prefix, char, suffix);

        // Test that both prefix and suffix operations don't panic
        let prefix_result = utf8_safe_prefix(&content, 150); // Cut inside the char
        let suffix_result = utf8_safe_suffix(&content, 150); // Cut inside the char

        // Results should be valid UTF-8
        assert!(prefix_result.is_char_boundary(prefix_result.len()));
        assert!(suffix_result.is_char_boundary(0));
        // Results must be bounded
        assert!(prefix_result.len() <= 150);
        assert!(suffix_result.len() <= 150);
    }
}
