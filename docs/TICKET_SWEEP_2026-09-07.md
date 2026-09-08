# Ticket sweep, 7 September 2026

Work is integrated on branch `codex/sept-ticket-sweep` in
[PR #1137](https://github.com/Kh1ng/git-agent-harness/pull/1137). It builds on
`5a2be52`, the Windows GUI and WSL onboarding change. Merge requires the
final revision to pass CI. No release has been published.

## Implemented

| Ticket | Result | Focused evidence |
| --- | --- | --- |
| #1112 | Explicit backend and model selections cannot silently fall back. Auto routing retains its existing fallback behavior. | 68 routing tests, 2 operator-pin CLI tests, 9 server session tests; server typecheck. |
| #1116 | Chat, archive, issue, and PR lists share search, a 10-row default, selected-row retention, and reveal controls. | 4 original component checks; expanded to 5 with failure recovery and native dialog behavior. |
| #1074 | Dock, tray, and launch-window preferences persist. No-icon startup opens a window; native exit reaps an owned non-Windows worker. | 2 desktop Rust tests, TypeScript and Vite checks, mocked Mac/Windows browser checks. |
| #946 | Nodes view shows cached health, age, profiles, resources, claims, and central leases, with live checks and registration. | 34 registry/liveness/claims tests, 1 WebSocket invalidation test, 3 component checks. |
| #1115 | Initial worker context keeps the task, project rules, and repair evidence. A session-local repository map and background sources are deferred, with separate accounting. | 73 focused Rust context, prompt, repair, and review opt-in tests. |

The UI audit also fixed shared text contrast, keyboard focus, modal behavior,
issue/PR error recovery, and eager secondary-page imports. See
[the scoped Impeccable report](UI_AUDIT_2026-09-07.md) for measurements and
remaining UI work.

Settings now uses the same quota snapshot as Quota for backend eligibility
(#760), including observation times and refresh failures. Profile changes
reject late responses. SCM authentication controls remain separate. See
[the testing audit](TESTING_AUDIT_2026-09-07.md) for coverage gaps found and fixed.

## Design choices

- Backend selection stays in routing. Removing unreachable explicit-fallback
  construction also removes a second interpretation of operator intent.
- One collection component owns filtering, the row limit, and selection
  retention. Callers supply records and render rows.
- Shared context preparation owns repository-map creation, prompt budgeting,
  and delivered/deferred accounting. Callers do not rebuild those steps.
- RegistryService owns observation caching and request ordering. The UI refetches
  authenticated snapshots after payload-free WebSocket notifications; it adds no
  health poller. Claims reflect the named observed profile; central leases cover
  all profiles.
- Native dialogs own page inertness, keyboard handling, and focus restoration.
  The app owns whether a dialog is open.
- React lazy loading uses the existing loading component. No new dependency
  or custom page-loading framework was added.

These are the APOSD checks applied to the changes. This pass did not perform
or claim a quantitative whole-repository APOSD score.

## Integration verification

After cherry-picking the agents' changes, contracts/shared builds and all
three application TypeScript checks passed. Web and desktop frontend builds
passed. The initial web JavaScript chunk is 338.00 kB, down from 515.57 kB.

The combined browser run passed 11 component checks and 8 smoke checks across
five viewport sizes. The smoke routes include Nodes. Fleet publication and
snapshot checks passed again after integration, as did all 9 session tests.
Mock-server socket reset messages during browser teardown did not fail tests.

See [the backlog triage](BACKLOG_TRIAGE_2026-09-07.md) for the next eight
priorities and requirements that need reconciliation before implementation.

## Verification limits

Rust builds ran one at a time with one compiler job and serial test execution.
Full Rust tests and all-target, all-feature Clippy still belong in CI. The
Mac had about 67 GiB free after this work; existing built apps were preserved.

Windows GUI launch, WSL install/reboot, forwarded reachability, CLI readiness,
and switching between computers still need the user's Windows machine.
Native Dock/tray transitions need a packaged-app check. QR pairing and mobile
packaging remain planned work. The current iOS/Android goal is control only.

## Manual work assignments

- Vibe: #1124 Overview navigation in `gah-overview-1124`.
- AGY 2: iOS control-client testing in `gah-ios-testing`.
- Claude on Windows: the GUI/WSL onboarding and readiness flow. The existing
  Windows onboarding handoff remains valid; use the integrated sweep branch
  to test the later desktop settings and UI changes too.

Keep each task in its assigned worktree. Do not ask two tools to edit the
integration worktree at once. The earlier Windows onboarding bundle is a
frozen checkpoint; the sweep has its own bundle.
