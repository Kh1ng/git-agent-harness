# TICKET-092-FIX: Fix YAML frontmatter parsing in parse_yaml_frontmatter

Goal: Fix two bugs in parse_yaml_frontmatter discovered during review:
1. split_once(':') truncates values containing colons (e.g. "TICKET-058: Descriptive MR Titles")
2. find("---") matches substring anywhere, not just on its own line

Difficulty: easy
Risk: low
Recommended backend: agy
Recommended model: Gemini 3.5 Flash (Medium)

## Affected Files
- src/dispatch.rs

## Acceptance Criteria
1. YAML values containing colons parse correctly (e.g. title: "TICKET-058: fix")
2. Frontmatter closing delimiter must be "---" on its own line
3. Existing tests still pass
4. New test for colon-containing YAML values
5. New test for --- delimiter only on own line

## Verification Commands
- cargo fmt --check
- cargo test
