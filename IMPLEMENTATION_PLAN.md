# GAH Stabilization Implementation Plan

## Status: ✅ Supervisor Blocker RESOLVED | 📋 Design Phase

**Prerequisites Complete:**
- ✅ GitHub `statusCheckRollup: null` sync failure FIXED
- ✅ Regression tests added and passing
- ✅ Release binary rebuilt
- ✅ `cargo fmt --check` passes
- ✅ Web app build artifacts added to .gitignore
- ✅ Vibe and OpenCode review backends enabled

---

## 1. SHA-Based Review Deduplication

### Problem Statement
Currently, GAH can review the same unchanged work multiple times, wasting budget. The desired invariant:
```
same ticket + same MR/PR + same head SHA + same reviewer class = no duplicate review
```

### Current State Analysis

**Existing Infrastructure:**
- ✅ `ReviewTarget` struct with `source_branch`, `target_branch`, `url`, `id`
- ✅ `LedgerEntry` with `work_id`, `branch`, `mr_url` 
- ✅ `check_duplicate_work()` prevents dispatch when active PRs exist for same work_id/branch
- ✅ `ReviewTarget` fields: `id`, `url`, `source_branch`, `target_branch`, `title`, `body`, `ci_status`
- ❌ No SHA tracking in any existing structures
- ❌ No review-specific deduplication (only work dispatch deduplication)

**Current Protection Gaps:**
- `check_duplicate_work()` only prevents **dispatch** of new work, not **re-review** of existing work
- No SHA-based tracking means same commit can be reviewed multiple times
- No reviewer class tracking means same backend/model can re-review same work

### Proposed Implementation

#### Small Issue 1: Add SHA Tracking to ReviewTarget
**Scope:** Minimal, backwards-compatible
**Files:** `src/provider.rs`
**Changes:**
```rust
// Add to ReviewTarget struct
pub source_sha: Option<String>,
pub target_sha: Option<String>,
```

**Migration Impact:**
- `#[serde(default)]` ensures existing entries deserialize
- Fields are `Option<String>` so missing data = None, not error
- No breaking changes to existing code

#### Small Issue 2: Add Review Deduplication Index
**Scope:** Small, uses existing ledger infrastructure
**Files:** `src/ledger.rs`, `src/dispatch.rs`
**Changes:**
```rust
// New function to check for existing reviews
pub fn review_already_exists(
    cfg: &GahConfig,
    work_id: &str,
    source_sha: &str,
    reviewer_class: &str,  // e.g., "strong", "weak", or specific backend+model
) -> Result<bool> {
    // Query ledger entries for same work_id
    // Check if any have matching source_sha and reviewer_backend/model
    // Return true if duplicate found
}
```

**Integration Points:**
- Call from `dispatch::review()` before `run_review_backend()`
- Log `review_skipped_duplicate` event if duplicate detected
- Preserve existing review attempt caps and fallback logic

#### Small Issue 3: Enhance Reviewer Identity Tracking  
**Scope:** Small, builds on existing fields
**Files:** `src/ledger.rs`, `src/dispatch.rs`
**Changes:**
```rust
// Add reviewer_class field to LedgerEntry
pub reviewer_class: Option<String>,  // "strong", "weak", or backend-specific
```

**Logic:**
```rust
// In apply_route_to_ledger or review preprocessing
fn determine_reviewer_class(route: &RouteDecision) -> String {
    if route.effective_backend == profile.routing.strong_review_backend.as_deref() {
        "strong".to_string()
    } else if route.effective_backend == profile.routing.weak_review_backend.as_deref() {
        "weak".to_string()
    } else {
        format!("{}:{}", route.effective_backend, route.effective_model.as_deref().unwrap_or(""))
    }
}
```

### Event Types to Add
```rust
// In src/events.rs EventType enum
ReviewSkippedDuplicate,
```

### Configuration Hooks
None needed - uses existing routing configuration

### Backward Compatibility
- All new fields use `Option<T>` with `#[serde(default)]`
- Existing entries deserialize without new fields
- New logic only activates when new fields are populated

---

## 2. Explicit Review-Cycle Caps

### Problem Statement  
Need hard caps to prevent unbounded review loops and budget exhaustion:
- Max review/fix cycles per ticket
- Max paid reviews per ticket
- Controller-owned retry/re-review policy

### Current State Analysis

**Existing Infrastructure:**
- ✅ `MAX_REVIEW_ATTEMPTS = 3` in `dispatch.rs:2344` - limits backend retries for review
- ✅ `STUCK_LOOP_THRESHOLD = 3` in `controller.rs:362` - detects repeated action selections
- ✅ `count_fix_attempts_per_branch()` in `sync.rs:396` - tracks fix attempts
- ✅ `count_merge_attempts_per_branch()` in `sync.rs:421` - tracks merge attempts
- ✅ `attempts_started` and `attempts_completed` in `LedgerEntry`
- ✅ `attempts: Vec<AttemptRecord>` tracks individual attempt details

**Current Protection Gaps:**
- Review caps are per **dispatch attempt**, not per **ticket lifecycle**
- No prevention of re-reviewing same work after successful review
- No explicit paid review budget tracking
- Models can potentially trigger unbounded self-review loops

### Proposed Implementation

#### Small Issue 4: Add Review Cycle Tracking to Ledger
**Scope:** Minimal, uses existing ledger fields
**Files:** `src/ledger.rs`
**Changes:**
```rust
// Add to LedgerEntry
#[serde(default)]
pub review_cycle_count: u32,  // Increment each time a review is attempted for this work_id

#[serde(default)]  
pub paid_review_count: u32,  // Increment each time a paid backend is used for review
```

**Integration Points:**
- Increment in `apply_route_to_ledger()` when `mode == "review"`
- Check caps before dispatch in `dispatch::review()`

#### Small Issue 5: Add Review Budget Configuration
**Scope:** Small, backwards-compatible
**Files:** `src/config.rs`
**Changes:**
```rust
// Add to RoutingPolicy
#[serde(default)]
pub max_review_cycles_per_ticket: Option<u32>,  // Default: 2

#[serde(default)]  
pub max_paid_reviews_per_ticket: Option<u32>,  // Default: 3
```

#### Small Issue 6: Implement Review Budget Checks
**Scope:** Small, uses existing controller logic
**Files:** `src/dispatch.rs`
**Changes:**
```rust
// Add to dispatch::review()
fn check_review_budget(cfg: &GahConfig, ledger_entries: &[LedgerEntry], work_id: &str) -> Result<()> {
    let max_cycles = profile.routing.max_review_cycles_per_ticket.unwrap_or(2);
    let max_paid = profile.routing.max_paid_reviews_per_ticket.unwrap_or(3);
    
    let recent_reviews = ledger_entries.iter()
        .filter(|e| e.work_id.as_deref() == Some(work_id) && e.mode == "review")
        .collect::<Vec<_>>();
    
    let cycle_count = recent_reviews.len() as u32;
    let paid_count = recent_reviews.iter()
        .filter(|e| is_paid_backend(&e.effective_backend))
        .count() as u32;
    
    if cycle_count >= max_cycles {
        anyhow::bail!("Review cycle budget exhausted: {} >= {}", cycle_count, max_cycles);
    }
    if paid_count >= max_paid {
        anyhow::bail!("Paid review budget exhausted: {} >= {}", paid_count, max_paid);
    }
    
    Ok(())
}
```

### Event Types to Add
```rust
ReviewBudgetExhausted,
```

### Configuration Hooks
- New optional fields in `RoutingPolicy` with sensible defaults
- Inherits from defaults → repo → profile hierarchy

### Backward Compatibility
- New config fields are `Option<T>` with defaults
- Existing configs continue working with default values
- New checks only activate when configured

---

## 3. Spend/Resource Telemetry

### Problem Statement
Need comprehensive spend and resource tracking for model + harness evaluation:
- Input/output/cached tokens
- Estimated cost
- Wall time
- Attempt count
- Resource usage (RSS, CPU if measurable)

### Current State Analysis

**Existing Infrastructure:**
- ✅ `LedgerUsage` struct with comprehensive fields:
  - `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`
  - `total_tokens`, `requests_count`
  - `estimated_cost_usd`, `actual_cost_usd`
  - `quota_window`, `quota_used_percent`, `quota_remaining_percent`, `quota_reset_at`
- ✅ `AttemptRecord.usage` tracks per-attempt usage
- ✅ `LedgerEntry.usage` tracks aggregate usage
- ✅ `CandidateConfig` has `marginal_cost_usd`, `quota_pool`, `quota_usage_percent`
- ✅ Duration tracking via `duration_seconds` in both `LedgerEntry` and `AttemptRecord`
- ✅ Attempt counting via `attempts_started`, `attempts_completed`

**Current Protection Gaps:**
- Usage fields exist but may not be consistently populated
- No explicit tracking of which backend/model was responsible for usage
- No association between usage and specific review cycles
- No resource usage (RSS, CPU) tracking

### Proposed Implementation

#### Small Issue 7: Ensure Usage Data Consistency
**Scope:** Small, data quality improvement
**Files:** `src/dispatch.rs`, `src/runner.rs`
**Changes:**
- Audit existing usage population in `run_review_backend()` and other backend runners
- Ensure `LedgerUsage` fields are properly populated from backend output
- Add fallback estimation when backend doesn't report usage

#### Small Issue 8: Add Usage Aggregation by Model/Backend
**Scope:** Small, analytics enhancement  
**Files:** `src/ledger.rs`
**Changes:**
```rust
// Add aggregation functions
pub fn usage_by_backend(entries: &[LedgerEntry]) -> HashMap<String, LedgerUsage> {
    let mut by_backend = HashMap::new();
    for entry in entries {
        *by_backend.entry(entry.effective_backend.clone()).or_default() += entry.usage;
    }
    by_backend
}

pub fn usage_by_model(entries: &[LedgerEntry]) -> HashMap<String, LedgerUsage> {
    let mut by_model = HashMap::new();
    for entry in entries {
        if let Some(model) = &entry.effective_model {
            *by_model.entry(model.clone()).or_default() += entry.usage;
        }
    }
    by_model
}
```

#### Small Issue 9: Add Resource Usage Tracking (Optional)
**Scope:** Small, platform-specific
**Files:** `src/runner.rs`
**Changes:**
```rust
// Add to LedgerEntry
#[serde(default)]
pub max_rss_mb: Option<u64>,  // Maximum resident set size

#[serde(default)]
pub cpu_seconds: Option<f64>,  // Total CPU time used
```

**Platform Support:**
- Linux: Parse `/proc/self/status` for RSS, `/proc/self/stat` for CPU
- macOS: Use `sysctl` or `ps` commands
- Windows: Use `GetProcessMemoryInfo` via FFI or subprocess
- Fallback: Leave as None if unavailable

### Event Types to Add
```rust
UsageThresholdExceeded,
ResourceLimitExceeded,
```

### Configuration Hooks  
- Add optional `max_ticket_cost_usd` field to `RoutingPolicy`
- Add optional `max_run_cost_usd` field to `RoutingPolicy`
- Add optional `max_wall_time_seconds` field to `RoutingPolicy`

### Backward Compatibility
- All new fields are `Option<T>` with `#[serde(default)]`
- Existing entries work without new fields
- New aggregations work with partial data

---

## 4. Model + Harness Evaluation Logging

### Problem Statement
Need structured telemetry to compare model + harness combinations:
- Work identity (ticket, task class, repo, SHA)
- Harness identity (separate from backend)
- Model identity
- Runtime metrics
- Usage metrics  
- Behavior metrics
- Outcome metrics

### Current State Analysis

**Existing Infrastructure:**
- ✅ Comprehensive `LedgerEntry` with work identity: `work_id`, `repo_id`, `repo`, `mode`
- ✅ Backend/model tracking: `backend`, `effective_backend`, `requested_model`, `effective_model`
- ✅ Runtime metrics: `duration_seconds`, `timestamp`
- ✅ Usage metrics: `LedgerUsage` with tokens, costs, quota info
- ✅ Outcome tracking: `validation_result`, `commit_created`, `mr_created`, etc.
- ✅ Events system: `ControllerEvent` with `event_type`, `timestamp`, `profile`, `work_id`, `details`
- ❌ No explicit harness identity (separate from backend)
- ❌ No task class/difficulty tracking
- ❌ No behavior metrics (tool calls, file edits, etc.)
- ❌ No outcome metrics (tests passed, etc.)

**Current Protection Gaps:**
- Harness and backend are conflated in current tracking
- No separation of model effect vs harness effect
- Limited behavior tracking

### Proposed Implementation

#### Small Issue 10: Add Harness Identity Tracking
**Scope:** Small, clarifies existing backend field
**Files:** `src/config.rs`, `src/ledger.rs`
**Changes:**
```rust
// Add to CandidateConfig
#[serde(default)]
pub harness: Option<String>,  // "opencode", "openhands", "codex", "agy", "claude-code", etc.

// Add to LedgerEntry  
#[serde(default)]
pub harness: Option<String>,  // Harness used for this run

// Add to ReviewTarget
#[serde(default)]
pub harness: Option<String>,  // Harness that created/manages this target
```

**Logic:**
```rust
// In RouteDecision and ledger population
fn determine_harness(backend: &str, model: Option<&str>) -> Option<String> {
    // Map known backend+model combinations to harness identities
    match (backend, model) {
        ("claude", _) => Some("claude-code".to_string()),
        ("codex", _) => Some("codex".to_string()),  
        ("agy", _) | ("agy-main", _) | ("agy-second", _) => Some("agy".to_string()),
        ("vibe", _) => Some("vibe".to_string()),
        ("opencode", _) => Some("opencode".to_string()),
        ("openhands", _) | ("cloud-coder", _) | ("auto", _) => Some("openhands".to_string()),
        _ => None,
    }
}
```

#### Small Issue 11: Add Task Classification Fields
**Scope:** Small, metadata enhancement
**Files:** `src/dispatch.rs`, `src/ledger.rs`
**Changes:**
```rust
// Add to LedgerEntry
#[serde(default)]
pub task_class: Option<String>,  // "improve", "fix", "pm", "review", "experiment"

#[serde(default)]  
pub difficulty: Option<String>,  // "easy", "medium", "hard" - from ticket metadata
```

#### Small Issue 12: Add Behavior Metrics to Ledger
**Scope:** Small, uses existing runner output parsing
**Files:** `src/ledger.rs`
**Changes:**
```rust
// Add to LedgerEntry
#[serde(default)]
pub tool_calls_count: Option<u32>,

#[serde(default)]
pub shell_calls_count: Option<u32>,

#[serde(default)] 
pub file_edits_count: Option<u32>,

#[serde(default)]
pub test_runs_count: Option<u32>,
```

**Integration:**
- Parse from backend output logs (stdout/stderr)
- Add to `apply_route_to_ledger()` or `finalize_ledger_entry()`

#### Small Issue 13: Add Outcome Metrics
**Scope:** Small, builds on existing fields
**Files:** `src/ledger.rs`
**Changes:**
```rust
// Add to LedgerEntry
#[serde(default)]
pub tests_passed: Option<bool>,

#[serde(default)]
pub reviewer_approved: Option<bool>,

#[serde(default)]
pub human_accepted: Option<bool>,
```

### Event Types to Add
```rust
ModelEvaluationRecorded,
HarnessEvaluationRecorded,
```

### Configuration Hooks
None needed - uses existing metadata parsing from tickets

### Backward Compatibility
- All new fields are `Option<T>` with `#[serde(default)]`
- Existing entries deserialize without new fields
- New metrics populate when data is available

---

## 5. Controller Decision Points Analysis

### Key Decision Points for Integration

1. **`decide_next_action()`** (`controller.rs:160`)
   - Determines what action to take based on status snapshot
   - **Integration Point:** Add review budget checks here

2. **`check_duplicate_work()`** (`dispatch.rs:144`)
   - Prevents duplicate work dispatch
   - **Integration Point:** Add SHA-based review deduplication here

3. **`dispatch::review()`** (`dispatch.rs:2340+`)
   - Main review dispatch function
   - **Integration Points:**
     - Add review budget checks before review attempt
     - Add SHA-based deduplication check
     - Populate harness identity

4. **`run_review_backend()`** (`runner.rs:755`)
   - Executes review backend
   - **Integration Points:**
     - Ensure usage data is properly captured
     - Add resource tracking
     - Parse behavior metrics from output

5. **`apply_route_to_ledger()`** (`dispatch.rs:5950`)
   - Records ledger entries for actions
   - **Integration Points:**
     - Populate all new telemetry fields
     - Increment review cycle counters
     - Update usage aggregations

6. **`detect_stuck_loop()`** (`controller.rs:367`)
   - Detects repeated action selections
   - **Integration Point:** Existing, may need tuning for review-specific cases

---

## Implementation Priority & Dependencies

### Phase 1: Foundation (High Priority)
1. **Small Issue 1:** Add SHA tracking to ReviewTarget
2. **Small Issue 4:** Add review cycle tracking to Ledger
3. **Small Issue 10:** Add harness identity tracking

### Phase 2: Safety (High Priority)  
4. **Small Issue 2:** Add review deduplication index
5. **Small Issue 5:** Add review budget configuration
6. **Small Issue 6:** Implement review budget checks
7. **Small Issue 7:** Ensure usage data consistency

### Phase 3: Telemetry (Medium Priority)
8. **Small Issue 8:** Add usage aggregation by model/backend
9. **Small Issue 11:** Add task classification fields
10. **Small Issue 12:** Add behavior metrics to Ledger
11. **Small Issue 13:** Add outcome metrics
12. **Small Issue 9:** Add resource usage tracking (optional)

### Phase 4: Polish (Low Priority)
- Add remaining event types
- Add configuration UI/CLI for new limits
- Add documentation
- Add integration tests

---

## Migration & Backward Compatibility

### Schema Evolution Strategy
All new fields use the established pattern:
```rust
#[serde(default)]
pub new_field: Option<T>,
```

This ensures:
1. **Backward Compatibility:** Existing JSONL ledger files continue to deserialize
2. **Forward Compatibility:** New code can handle old data (None values)
3. **No Breaking Changes:** No changes to existing enum variants or required fields

### Data Migration
No migration needed:
- Old entries have new fields as `None`
- New logic checks for `None` and provides defaults
- Aggregations and queries handle missing data gracefully

### Version Compatibility
- Minimum supported config version: unchanged
- Ledger file format: unchanged (JSONL with optional new fields)
- No database migrations required

---

## Testing Strategy

### Unit Tests
- Test new deduplication logic with various SHA/work_id combinations
- Test budget checking with different configuration values
- Test usage aggregation functions
- Test harness identity mapping

### Integration Tests  
- Test end-to-end review deduplication flow
- Test budget exhaustion scenarios
- Test telemetry data collection and reporting

### Regression Tests
- Ensure existing behavior unchanged when new features disabled
- Test backward compatibility with old ledger entries
- Test graceful degradation when new fields missing

---

## Configuration Examples

### Review Budget Configuration
```toml
[routing]
max_review_cycles_per_ticket = 2
max_paid_reviews_per_ticket = 3

# Per-backend overrides possible
[profiles.my-repo.routing] 
max_review_cycles_per_ticket = 5  # More cycles for important repo
```

### Spend Limits Configuration
```toml
[routing]
max_ticket_cost_usd = 50.00
max_run_cost_usd = 25.00
max_wall_time_seconds = 3600  # 1 hour
```

---

## Summary

### What's Ready Now
- ✅ Supervisor blocker resolved
- ✅ Vibe and OpenCode review backends enabled
- ✅ Build artifacts properly ignored
- ✅ Code formatting clean

### What's Designed and Ready for Implementation
- **12 small, coherent issues** that can be implemented incrementally
- **All backwards-compatible** with established schema evolution patterns
- **Reuses existing infrastructure** (ledger, events, configuration)
- **Clear integration points** identified
- **Comprehensive testing strategy** outlined

### Recommended Next Steps
1. Implement Phase 1 (Foundation) issues first
2. Test each issue independently
3. Proceed to Phase 2 (Safety) once foundation stable
4. Implement Phase 3 (Telemetry) for evaluation capabilities
5. Monitor and tune based on real usage data

### Estimated Effort
- Each small issue: 1-4 hours implementation + 1-2 hours testing
- Total Phase 1: ~2-3 days
- Total Phase 2: ~3-4 days  
- Total Phase 3: ~2-3 days
- **Total: ~1-2 weeks** for full stabilization feature set

---

## Appendix: Existing Backend/Controller Integration

### Supported Review Backends
| Backend | Command | Review Args | Status |
|---------|---------|------------|--------|
| claude | `claude` | `-p <prompt>` + profile args | ✅ Active |
| codex | `codex` | `exec <prompt>` + model args | ✅ Active |
| agy/agy-main/agy-second | `agy` | `--print <prompt> --model <model>` | ✅ Active |
| vibe | `vibe` | `review <prompt> --model <model>` | ✅ **NEW** |
| opencode | `opencode` | `review <prompt> --model <model>` | ✅ **NEW** |

### Controller Event Types (Current)
- ObservationCompleted
- ActionDecided  
- ActionOverridden
- DispatchStarted
- DispatchFinished
- BackendMarkedUnavailable
- WaitSelected
- HumanRequired
- DuplicateGuardTriggered
- LoopStopped

### Proposed New Event Types
- ReviewSkippedDuplicate
- ReviewBudgetExhausted
- UsageThresholdExceeded
- ResourceLimitExceeded
- ModelEvaluationRecorded
- HarnessEvaluationRecorded

### Ledger Fields (Current vs Proposed)

#### Current Work Identity
- work_id, repo_id, repo, mode, target_summary
- branch, mr_url, session_id, session_dir

#### Current Backend/Model  
- backend, requested_backend, effective_backend
- requested_model, effective_model
- fallback_used, routing_reason

#### Current Usage Tracking
- duration_seconds
- usage (LedgerUsage with tokens, costs, quota)
- attempts (Vec<AttemptRecord>)

#### Proposed Additions
- **Deduplication:** source_sha, target_sha, reviewer_class
- **Budget:** review_cycle_count, paid_review_count  
- **Harness:** harness
- **Classification:** task_class, difficulty
- **Behavior:** tool_calls_count, shell_calls_count, file_edits_count, test_runs_count
- **Outcomes:** tests_passed, reviewer_approved, human_accepted
- **Resources:** max_rss_mb, cpu_seconds