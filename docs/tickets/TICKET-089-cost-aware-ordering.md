# TICKET-089: Cost-aware candidate ordering

Goal: Combine ordered candidates, quota pace, marginal cost, and configured priority into an explainable routing decision.

Difficulty: hard
Risk: medium
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Add cost-aware candidate ordering with quota pacing and marginal cost

## Problem

Current routing cannot consider cost or quota pressure. After TICKET-086 (ordered lists) and TICKET-088 (quota pacing), routing must combine:
- candidate order
- availability (hard filter)
- capability requirements (hard filter where defined)
- quota pace band
- marginal cash cost
- configured escalation policy

The same model can have different marginal cost depending on execution path:
- Codex gpt-5.4 via subscription: effectively zero marginal cash cost within quota
- OpenHands gpt-5.4 via paid API: real marginal cost
- Model name alone must not determine economic preference

## Acceptance Criteria

1. Availability remains hard eligibility filter
2. Capability remains a hard constraint where defined
3. Cost/quota affects ordering among eligible candidates
4. Under-pace subscription routes are favored (zero marginal cost)
5. Over-pace scarce subscription routes are conserved
6. Paid API equivalents should not beat included subscription route without a policy reason
7. Genuine agent failure may escalate (capability upgrade)
8. Environment/harness/auth/quota failures do not trigger capability escalation
9. Routing reason explains each reorder decision
10. Backward compatible: profiles without cost config use existing ordering

## Affected Files

- `src/routing.rs` — Cost/quota-aware ordering
- Profile config — Cost/policy parameters

## Constraints

- Dependencies: TICKET-086, TICKET-087, TICKET-088 must be complete first
- Do not add AGY support
- Do not broadly redesign backend instances
- Keep routing reason explanations structured

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
