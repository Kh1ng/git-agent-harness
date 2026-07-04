# TICKET-103: Separate Reviewer Strength from Review Verdict Confidence

Priority: P0
Difficulty: Medium
Status: TODO

## Summary

Separate the strength/trust tier of the reviewer from the reviewer's returned verdict confidence.

Current manager behavior conflates values such as `APPROVE_WEAK` with "weak reviewer." This is incorrect. A Claude Sonnet reviewer may be a strong reviewer while returning a cautious approval.

Example:
- reviewer_tier = strong
- verdict = approve
- confidence = weak
- blocking_findings = []
- human_required = false

This should not automatically become a human-review requirement merely because the model used the word WEAK.

## Goal

Model reviewer authority and review outcome as separate dimensions.

Conceptually:
- Reviewer identity/strength: strong, standard, weak
- Review result: approve, changes_required, reject
- Confidence: high, medium, low
- Human escalation: explicit boolean or policy result

## Requirements

- Inspect current review verdict schema and merge policy
- Identify where APPROVE_WEAK currently triggers escalation
- Separate reviewer strength from returned verdict/confidence
- Make reviewer strength policy-configurable
- Preserve backward compatibility with existing structured review output
- Avoid relying solely on free-text verdict strings for merge policy
- Ensure Claude Sonnet can be configured as a strong reviewer
- Keep human_required semantically independent

## Acceptance Criteria

- Claude Sonnet can be marked strong
- APPROVE_WEAK from a strong reviewer does not automatically imply weak reviewer identity
- No blockers + human_required=false behaves according to explicit policy
- Blocking findings still stop merge
- Weak reviewers cannot self-promote authority via high-confidence text
- Existing review flows remain functional

## Tests

- strong reviewer + strong approval
- strong reviewer + weak/cautious approval
- strong reviewer + blocker
- weak reviewer + strong approval
- explicit human-required
- malformed review result
- fallback reviewer
- backward compatibility with current verdict strings

## Constraints

- No hardcoded special-case if reviewer == Claude
- No weakening blocker semantics
- No automatic merge-policy redesign beyond this distinction
- No prompt-only fix
- No warning suppression

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`

## Report

Report: prior conflation, new policy model, migration/backward compatibility, affected files, tests, validation results.
