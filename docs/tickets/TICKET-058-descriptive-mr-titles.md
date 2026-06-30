# TICKET-058: Descriptive MR Titles

Goal: Improve MR title generation to include ticket context instead of generic "GAH: improve" tags.

Difficulty: easy
Risk: low
Recommended backend: codex

## Why This Is Uncovered
Current MR titles are generic `[GAH] improve: gah` — no ticket context. Makes reviewing confusing.

## Affected Files
- src/dispatch.rs

## Acceptance Criteria
1. The `TicketMetadata` struct gains a `title` field extracted from the first `# TICKET-XXX: Title` h1 heading in the ticket file.
2. `parse_ticket_metadata` extracts the title from `# TICKET-XXX: Title` format.
3. The MR title in `improve()` uses the ticket title when available (e.g. `[GAH] TICKET-058: Descriptive MR Titles`).
4. If validation fails, prefix stays `[GAH][DRAFT-FAIL]` with the ticket title.
5. Falls back to `[GAH] {mode}: {repo_id}` when no ticket file or no title found.
6. Also update the commit message to include the ticket label.
7. Existing test `parses_ticket_metadata_for_routing` still passes.
8. Add a test `ticket_metadata_extracts_title_from_h1` verifying title extraction.

## Verification Commands
- `cargo test parses_ticket_metadata_for_routing -p git-agent-harness 2>&1 | tail -5`
- `cargo test ticket_metadata_extracts_title_from_h1 -p git-agent-harness 2>&1 | tail -5`
- `cargo build --quiet 2>&1`
