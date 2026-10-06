# APOSD Audit: Deep-Module Review of Core Subsystems
Date: 2026-10-06

This audit evaluates the core subsystems of GAH based on A Philosophy of Software Design (APOSD) principles. The subsystems evaluated include Routing, Dispatch, Telemetry, and CLI.

## 5-Dimension Scores (aposd-audit)

The components are scored on a scale of 1-5 (5 being best) across 5 APOSD dimensions:

| Subsystem | Module Depth | Info Hiding | General Purpose | Error Handling | Cognitive Load |
|-----------|--------------|-------------|-----------------|----------------|----------------|
| Routing   | 3            | 4           | 3               | 4              | 4              |
| Dispatch  | 2            | 3           | 4               | 3              | 3              |
| Telemetry | 4            | 4           | 4               | 4              | 4              |
| CLI       | 2            | 3           | 3               | 4              | 2              |

## Top Issues by Severity

### P0/P1 Findings

1. **P1: Shallow module in Dispatch Review**
   - **Component:** `src/dispatch/review/mod.rs`
   - **Finding:** The `dispatch::review` module acts purely as a pass-through for `context` and `policy`, providing no meaningful abstraction or depth. This violates the "Deep Modules" principle.
   - **Resolution:** Filed as a linked follow-up issue [TICKET-1392](tickets/open/TICKET-1392-shallow-module-dispatch-review.md).

2. **P1: God Class / Large File in CLI Args**
   - **Component:** `src/cli/args.rs`
   - **Finding:** The module has grown to ~1500 lines, taking on too many responsibilities (parsing, validation, subcommands) without hiding information behind a simple interface.
   - **Resolution:** Filed as a linked follow-up issue [TICKET-1393](tickets/open/TICKET-1393-cli-args-refactor.md).

### P2/P3 Findings

- **P2: Over-specific naming in Routing Policy**
  - **Component:** `src/routing/policy.rs`
  - **Finding:** Some routing policy functions are overly specific to the exact current task structure rather than being general-purpose matching engines.

- **P3: Repeated error translation**
  - **Component:** `src/runner/mod.rs`
  - **Finding:** Error propagation relies heavily on `anyhow::anyhow!` strings rather than structured, typed error enums, making programatic handling difficult for callers.
