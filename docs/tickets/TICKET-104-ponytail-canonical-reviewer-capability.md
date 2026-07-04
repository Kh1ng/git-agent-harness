# TICKET-104: Make Ponytail a Canonical Reviewer Capability

Priority: P0
Difficulty: Medium
Status: TODO

## Summary

Make Ponytail part of the configured reviewer contract rather than relying on managers or prompts to remember to invoke it manually.

Ponytail was previously missing from Claude Code after an install/state change. Once reinstalled, `/ponytail` worked again. This exposed a reliability problem: Claude binary exists ≠ expected review capability is present.

The desired policy is that Claude review runs use Ponytail by default, and future review skills should be configurable without hardcoding literal slash commands throughout dispatch logic.

## Goal

Represent review capabilities declaratively.

Conceptually:
```
[reviewers.claude-sonnet]
backend = "claude"
model = "sonnet"
reviewer_tier = "strong"
required_capabilities = ["ponytail"]
review_capabilities = ["ponytail-review"]
```

The exact schema should follow existing config conventions.

## Requirements

- Inspect existing reviewer/backend config
- Add the smallest reusable capability/skill abstraction
- Configure Claude Sonnet review to require Ponytail
- Ensure actual review invocation activates the capability deterministically
- Do not rely on prose such as "please use ponytail"
- Preserve backend-specific invocation semantics
- Avoid assuming every host uses slash commands identically
- Do not implement every host unless needed now; design abstraction so literal Claude syntax does not become canonical

## Acceptance Criteria

- Claude review policy explicitly requires Ponytail
- Missing Ponytail does not silently degrade to ordinary review
- Review artifacts record the capability policy used where practical
- Existing non-Ponytail reviewers remain supported
- Capability configuration is reusable for future skills

## Tests

- required capability present
- required capability absent
- optional capability absent
- Claude review invocation activates expected capability
- normal Claude implementation path unaffected where appropriate
- config override behavior

## Constraints

- No hardcoded `/ponytail` scattered across call sites
- No assumption all agents use slash commands
- No plugin auto-install during review
- No shell-profile hacks
- No warning suppression
- Keep scope reviewable

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`

## Report

Report: capability abstraction, Claude invocation behavior, failure/degradation semantics, config changes, tests, validation results.
