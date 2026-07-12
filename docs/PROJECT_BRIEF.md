# GAH Agent-Harness Project Brief

## Purpose
GAH is an agentic coding control plane that provides a durable, inspectable, bounded development loop.

## Core Capabilities
1. Observe repository and controller state
2. Classify what needs attention  
3. Choose one safe next action deterministically
4. Route work to eligible backend/model
5. Execute in isolated worktree
6. Validate results
7. Preserve attempts, failures, usage, routing evidence
8. Create, review, update, reconcile PRs/MRs
9. Persist structured outcomes
10. Retry, reroute, wait, escalate, merge, or request human intervention per policy

## Architecture Principles
- **Deterministic controller**: Decision path must remain deterministic
- **LLM as implementer/reviewer only**: LLMs implement and review, but do not determine controller policy
- **Self-hosting**: GAH improves itself through its own bounded loop
- **Isolation**: Work occurs in isolated worktrees, never mutates executing checkout
- **Evidence preservation**: All attempts, failures, usage, and routing are preserved

## Repository Structure

### Core Rust Crate
- `src/main.rs`: CLI entry point
- `src/dispatch.rs`: Core dispatch logic and worktree management
- `src/controller.rs`: Controller decision logic
- `src/routing.rs`: Backend routing and availability
- `src/ledger.rs`: Attempt tracking and outcome persistence
- `src/context.rs`: Context budgeting and prompt construction
- `src/config.rs`: Configuration loading and merging
- `src/provider.rs`: Backend provider abstractions
- `src/validation_check.rs`: Validation gate implementation

### Configuration
- `~/.config/gah/config.toml`: User configuration (routing, backends, profiles)
- `~/.config/gah/canonical.toml`: Shared canonical policy (inherited by repos)
- `docs/MANAGER_MEMORY.md`: Operational memory (being replaced by this brief + live packs)

### State
- `$XDG_STATE_HOME/gah/availability.json`: Backend/model availability tracking
- `$XDG_STATE_HOME/gah/ledger/`: Attempt ledger and reconciliation logs
- `$XDG_STATE_HOME/gah/worktrees/`: Isolated worktree checkouts

## Backend Strategy

### Codex
- Role: Reliable implementation, conflict resolution, ordinary-to-complex coding
- Route-aware model launch: resolved route identity overrides stale CLI flags
- Current models: `gpt-5.4-mini`, `gpt-5.6-luna`

### Claude
- Role: Strong review, architecture, adversarial evaluation, difficult work
- Reviewer tiers: `strong`, `standard`, `weak` (configured, not self-promoted)
- Required capabilities verified at preflight

### AGY (Antigravity)
- Role: Implementation backend with isolated instances
- Current versions: `agy 1.0.16`
- Logical backends: `agy`, `agy-main`, `agy-second`
- Instance isolation via distinct HOME/state roots and wrappers

### Vibe
- Role: Mistral-based implementation (local or API)
- Models: Configured per profile

### OpenCode
- Role: Open-source model backend (optional)
- Configuration: Per-profile when explicitly enabled

## Routing Policy

### Deterministic Rules
- Ordered route candidate lists per profile
- Availability-aware skipping (quota, auth, temporary failures)
- Cost-aware candidate ordering (marginal cost, not just model name)
- Failure-aware escalation (agent-capability failures only)

### Economic Preference
- Subscription-backed usage may have zero marginal cash cost
- API-backed use of same model may have real marginal cost
- Route resolution considers execution path economics

### Escalation Rules
- **Do NOT escalate** for: harness errors, environment errors, auth failures, quota exhaustion
- **Only escalate** for: genuine agent-performance failures where policy permits

## Work Identity and State

### Unique Work Identity
- Durable unique work identity across: active work, completed work, planned work, manager memory, ticket files
- Collision detection is a hard bug
- Synthetic branch-derived IDs only where no authoritative ticket exists

### State Hardening Chain
1. Ticket collision detection
2. Structured work metadata
3. Authoritative PR title/description generation
4. Ledger work identity propagation
5. Sync/reconciliation association by work identity
6. Duplicate-work guard

## Validation Policy

### Baseline Validation
- Must pass before implementation considered successful
- Classification: `harness_error`, `environment_error`, `backend_error`, `agent_no_progress`, `agent_failure`, `validation_failure`, `human_blocked`, `unknown`
- `harness_error` and `environment_error` stop work
- `unknown_red` stops by default unless explicitly overridden
- `expected_red` proceeds only when explicitly configured

### Validation Commands
- Do not weaken tests or suppress warnings to obtain green validation
- Store what backend reports; unknown remains unknown
- Never convert unknown to zero
- Do not normalize unlike provider concepts speculatively

## Merge Policy

### Autonomous Merge Conditions (all must pass)
- Implementation completed
- Validation passed
- No blocking review findings
- `human_required == false`
- No unresolved controller ambiguity
- No duplicate-work conflict
- Review policy requirements satisfied
- Required reviewer capabilities available
- PR/MR not superseded

### Human-Required States
- Explicit `human_required == true` stops automation
- Do not merge based on malformed, missing, or unparseable review output
- Do not treat empty backend output as success

## Context Budgeting

### Token Limits
- Soft limit: 80,000 tokens (default)
- Hard limit: 150,000 tokens (default)
- Per-profile and per-backend overrides supported

### Context Packs
1. **Project Brief**: This document (<= 2,500 tokens)
2. **Live Task Pack**: Generated per dispatch from typed state
3. **Retrieved Evidence**: On-demand, scoped to touched modules/diff
4. **Review Pack**: Diff, acceptance criteria, validation evidence, risk checklist

### Compaction Strategy
- Remove non-critical sections first: Manager Memory, Git History, Repository Map
- Protect critical sections: Focus, Acceptance Criteria, Verification Commands, Warning, Current Git, Unresolved
- Enforce budgets with explicit telemetry

## Coding Conventions

### Rust
- Match existing style (indentation, naming, error handling density)
- Minimal diff principle: remove completely when removing
- Update all call sites when making changes
- Comments only for *why*, not *what* (match file's existing comment density)

### TypeScript/JavaScript
- Follow existing project patterns in `apps/` and `packages/`
- Prefer explicit types over `any`
- Use async/await for I/O operations

### Commit Messages
- Follow conventional commits pattern
- Include issue references when applicable
- Keep subject line under 72 characters

## Command Reference

### Core Workflow
```bash
# Dispatch a specific issue
gah dispatch --target 286 --mode improve

# Run validation checks
gah validate --profile gah

# Check status
gah status --profile gah --json

# Review an MR
gah review --mr 286 --profile gah

# Run controller once
gah loop --once --profile gah
```

### Configuration Management
```bash
# Show effective configuration
gah config show --profile gah

# Edit configuration
gah config edit

# Show routing candidates
gah routing candidates --profile gah
```

### State Inspection
```bash
# Show availability state
gah availability --json

# Show ledger entries
gah ledger --json

# Show recent events
gah events --json --limit 20
```

### Development
```bash
# Build release binary
cargo build --release

# Run tests
cargo test --all-targets --all-features

# Run clippy
cargo clippy --all-targets --all-features -- -D warnings

# Format code
cargo fmt
```

## Failure Classification

### Failure Classes
- `harness_error`: GAH infrastructure failure
- `environment_error`: External environment issue (network, permissions)
- `backend_error`: Backend execution failure
- `agent_no_progress`: Agent made no meaningful progress
- `agent_failure`: Agent produced incorrect/incomplete output
- `validation_failure`: Post-implementation validation failed
- `human_blocked`: Explicit human intervention required
- `unknown`: Unclassified failure

### Failure Stages
- `preflight`, `baseline_validation`, `route`, `backend_launch`, `agent_run`, `post_validation`, `commit`, `push`, `mr_create`, `review`, `sync`

## Key Invariants

1. **Deterministic controller**: Same input state → same decision
2. **No unbounded recursion**: Bounded actions only
3. **Evidence preservation**: All attempts tracked in ledger
4. **Isolation**: Worktrees never mutate executing checkout
5. **Unique work identity**: No ticket ID collisions
6. **Fail-safe observation**: Critical observation failure stops safely
7. **No silent downgrades**: Reviewer capability requirements enforced
8. **Availability tracking**: Don't retry known-unavailable resources
9. **Appropriate escalation**: Only for agent-capability failures
10. **Token budgeting**: Context packs enforce limits

## Common Pitfalls

1. **Stale MANAGER_MEMORY**: Always prefer current structured state over prose
2. **Inferring healthy state**: Observation failure ≠ healthy empty state
3. **Backend identity confusion**: Use effective route identity, not CLI flags
4. **Over-escalation**: Don't escalate for quota/auth/environment issues
5. **Context bloat**: Use retrieval packs, don't inject full archives
6. **Test skipping**: Run full test suite, not just `--lib`

## Getting Help

- Check `gah --help` for command reference
- Review `docs/PROJECT_BRIEF.md` for architecture
- Consult `docs/OPERATIONS.md` for deployment and troubleshooting
- Examine ledger and events for failure diagnosis
- Use `gah doctor` for environment health checks
