# Testing audit, 7 September 2026

The suite had gaps in discovery, platform coverage, and failure evidence.
This pass fixes those gaps and adds checks for the affected behavior. It does
not claim a whole-repository coverage percentage.

## Findings and fixes

| Finding | Change | Evidence |
| --- | --- | --- |
| Server CI selected 40 of 43 source test files. The local cache runner used different discovery. | Both commands now use the recursive `apps/server/test.mjs` runner. | All 323 source tests passed locally; the previously omitted chat sessions, headless adapter, and preview proxy tests account for 38 checks. |
| Browser tests expected an old Add Node form and treated failed PR requests as empty results. | Updated assertions to the current install flow and explicit error/retry behavior. Disabled install inputs while submission is pending. | 34 component checks and 3 focused PR picker checks passed locally. Browser CI passed on revision `573aeec`. |
| An e2e failure prevented component checks and could lose failure artifacts. | Run components after e2e failure when setup succeeded; separate result directories; retain failure traces and screenshots. | Workflow and Playwright configuration inspected; the subsequent browser CI run passed both stages. |
| CI typechecked the web and mock server but omitted production server and MCP types. | Add their existing typecheck commands to the frontend build job. | Both pass locally; CI now gates them. |
| PR CI did not build native Windows and macOS bundles. | Desktop workflow now builds Windows NSIS and macOS apps for affected PRs and runs desktop unit checks. | macOS passed on `573aeec`. Windows caught an installer settings regression before packaging. The corrected PowerShell check passed on `3c04969`; native packaging was still running at this audit checkpoint. |
| Windows settings tests exposed JSON root conversion behavior. | Reject non-object JSON before PowerShell pipeline conversion; preserve existing settings on failure and retain unknown fields on successful updates. | Executable PowerShell regression covers malformed, array, null, and string roots plus valid settings. No local PowerShell runtime was available; the corrected check passed in Windows CI. |
| Rust scope matching hid unreachable filtering, while a naive fix would repeat unit tests through `--tests`. | Select the complete root suite once; run desktop tests for desktop changes. | Ten shell cases cover scope selection. Full Rust tests and Clippy remain CI gates. |
| Prompt compaction could discard the current failed attempt's validation evidence. | Protect current retry evidence in shared context preparation. | Regression failed before the fix; 2 CLI and 39 context checks passed afterward. |
| Session cancellation left a 30-second timer alive after successful cancellation. The timeout test used real time. | Clear the timer in `finally`; advance a fake clock across the timeout boundary. | All 9 session tests passed in about 0.3 seconds, previously about 35 seconds. |
| Settings showed boot-time backend discovery instead of routing eligibility. A late request could overwrite a newer profile. | Reuse Quota snapshots and reject obsolete requests by pending-resource identity. | Two browser scenarios cover eligibility, original timestamps, refresh failures, SCM refresh, and profile races. Removing the request guards makes the race test fail. Desktop and 390px rendering were inspected without horizontal overflow. |
| Node setup performed installer/archive work without a request limit. | Apply the existing rate limiter to the setup router. | Focused regression passes. Final CodeQL analysis remains a merge gate. |

## Coverage boundaries

The mock browser server tests UI contracts without dispatching paid agents or
modifying production state. Chromium coverage includes responsive smoke
checks, but it is not Safari, WebKit, or native mobile acceptance.

Windows still needs a real install, WSL enablement/reboot, GUI launch, service
restart, central connectivity, and CLI readiness check. Native Dock/tray
transitions need a packaged-app check. iOS and Android remain control surfaces;
QR pairing and device testing are not completed by this sweep.

The next worker-readiness task must distinguish HTTP reachability from a
profile that can access its repository and launch its configured backend.
Remote project import, remote chat, and native Windows agent execution remain
separate work.

Twenty-two Rust integration binaries repeat three shared support checks. They
are small; no coverage was removed to save those repetitions. Global Git
environment isolation and prompt-memory recall orchestration still need
separate investigation.

## Reproduction

- `npm test --workspace=apps/server`
- `npm run e2e --workspace=apps/web`
- `npm run test:component --workspace=apps/web`
- `bash scripts/test-ci-test-scope.sh`
- Windows: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-windows-installer.ps1`
- Root Rust: `cargo test --locked`; desktop: `cargo test --locked --manifest-path apps/desktop/Cargo.toml`

Use one local Rust compiler job on the development Mac. Final CI must pass on
the exact revision merged; earlier green runs do not validate later changes.
