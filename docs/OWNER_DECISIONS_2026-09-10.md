# Owner decision collection, 2026-09-10

Produced by the closing sweep that took the backlog from 58 open issues to 41
(closed with evidence: the wayfinder cluster #790–#800, #1074, #1076-adjacent
verifications, #116, #832, #833, #822, #532, #520, #834; partials landed under
#943). This document lists every open item that needs an owner decision, with a
recommended answer for each, so the next implementation stretch can run without
stopping for input.

Tickets not listed here are either in progress, blocked, or epics that stay
open by design.

## A. Control-plane / manager-integration series (parents #514, #515)

The series was filed before manager chat, the mutation API, and the session
log shipped. Several children are now done or nearly done; the real decisions
are about which remaining pieces are still wanted.

| Ticket | State today | Decision needed | Recommendation |
| --- | --- | --- | --- |
| #516 shared chat UI | Session list, streaming, steering, interrupts, approvals, resume, PR/work links, event-cursor reconnect all shipped in the React client; desktop uses the same app in the Tauri webview; mobile uses the responsive browser app. | Mobile notifications and deep links are the only unmet AC. | **Close #516**; move the mobile-notification remainder to #941 where notification parity already lives. |
| #517 operational UI parity | The shared React app covers profiles, routing, work, telemetry, quota, events, claims, loop lifecycle, approvals (GRANT/REVOKE #1167), backend toggles (#1175). | Which workflows are still "not every normal operator workflow"? | **Close with a verification pass**: enumerate the AC list against current pages and close, moving any single real gap to its own ticket. |
| #519 typed read API parity | Read API audit (2026-09-08, docs/READ_API_AUDIT_2026-09-08.md) lists concrete missing adapters: `telemetry.aggregate`, `claims.list`, `external_approval.inspect`, `quota.list`; CLI-first `telemetry.status`, `policy.check`, `profile.show`, price checks; capability manifest payload schemas. | Fund the remaining adapters now, or accept CLI-only for the long tail? | **Fund a bounded follow-up**: the four JSON-ready adapters (aggregate, claims.list, quota.list, approval inspect) are each small; the CLI-first four stay CLI-only. |
| #525 MCP tools | 21 tools shipped (reads + holds/dispatch/clear-attempts) through the authenticated API. | Finish manifest-generated tool schemas and approval-request tools? | **Finish as a small follow-up** (schemas from the manifest; add route-approval request/grant tools) — otherwise the hand-written schemas drift as the manifest grows. |
| #529 shared generated client | `packages/contracts` + hand-written `client.ts` with reconnect refresh work; a *generated* client does not exist. | Is the generated-client refactor wanted, or is the hand-written client the accepted steady state? | **Accept hand-written as steady state** until a second consumer (mobile shell) actually needs generation; reopen generation only then. |
| #530 manager chat CLI | No `gah manager chat` terminal command exists; the webUI chat is the operator surface and the Rust session layer exists underneath. | Build the terminal client or accept webUI-only? | **Accept webUI-only** for now; the Rust session layer makes the CLI client a bounded future addition when an operator actually needs headless chat. |
| #533 durable sessions/event inbox | Session log spine, compaction, usage shipped. Missing: idempotent event inbox, profile/session lease, cursor-resume across manager restarts. | Is multi-consumer dedup a live problem (one operator today)? | **Defer until a second manager consumer exists**; single-operator leases are speculative hardening. |
| #534 mobile PWA | Not started (icons exist, no manifest/service worker). | Priority vs native shells (#526/#936)? | **Do it after #529's decision** — it is the cheapest real mobile win and needs no app store. |
| #526 native packaging | CI builds desktop artifacts; iOS workflow exists for build validation. | Priority between PWA (#534) and native shells? | **PWA first** (above); native shells only after the PWA proves the mobile surface. |
| #539 resumable onboarding | Not started. | Priority? | **Keep parked** until a second real node onboards again; document the manual flow meanwhile. |
| #514 / #515 epics | Umbrellas. | Keep open as umbrellas? | **Keep open** but prune their child lists to reflect the closures above. |

## B. Product features awaiting a call

| Ticket | Decision needed | Recommendation |
| --- | --- | --- |
| #558 provider-native PM decomposition | PM plan generation/publication shipped (PM plan API, fingerprint-gated publish). What remains is provider-native issue authorship without secondary IDs. | **Keep open, medium priority**; it removes the dual-ID bookkeeping the PM flow still has. |
| #653 Telegram approvals (P0) | Which surface: Telegram bot (new dependency, credential storage, polling) vs dashboard push notifications? | **Build the dashboard/notification layer first (#941)**; add Telegram only if approvals genuinely need to be resolvable off-device. |
| #741 canonical readiness | Blocked by the #938 contract correction. | See #938 below. |
| #938 role contract | The doc says "worker has no server" but workers run one for remote dispatch. | **Correct the contract doc** to match reality (workers run a dispatch service; central owns TDAI DB + skill bank), then #741 implements truthful readiness against it. |
| #835 backend Phase 4 (node/fleet awareness in Rust) | Scope or park? | **Park** — the TS fleet layer (registry, #946 nodes view, #968 worker selection) already answers today's needs. |
| #863 Hermes/Cursor dispatch backends | Hermes is a real dispatch backend; Cursor is not. | **Split**: close the Hermes half as done, keep Cursor behind its own owner decision (is a Cursor adapter wanted at all?). |
| #830 shared dispatch memory | The mechanism shipped (memory hooks per tool; gateway client with dispatch-side leaf namespaces `gah:worker:{project}:{ticket}`). **Verified 2026-09-10: hooks are NOT installed on this macOS worker** (`~/.config/gah/memory-hooks.json` absent; no hook refs in `~/.claude/settings.json`), so sessions here capture nothing to the shared store. | Run `gah setup memory-hooks --tool claude,codex` on each active worker (one command; mutates the agent hook configs, hence owner's call), then verify captures land in the gateway and close. |
| #940 usage/spend tracking | Cost/telemetry/quota pages exist; per-API-key "bang-for-buck" rollups don't. | **Keep open, low priority**; the data is already in the ledger, so this is aggregation work when wanted. |
| #1076 Factory tab | Gated on "factory config landing" — which has not been designed. | **Decide the factory model first** or retitle this as the UX redesign ticket it actually is. |
| #1077 Waypoint/chartr visual tracking | Research done (docs/CHARTR_MAP_RESEARCH_2026-09-09.md). | **Decide after #799** below. |

## C. Pure owner decisions (one question each)

1. **#799 planning map format** — chartr-compatible files as optional input with GAH-native storage, per the research doc's suggestion? (Recommendation: yes, keep #799 as the decision venue.)
2. **#864 pair GitHub Copilot with opencode** — worth adding as a provider pairing like deepseek/glm today? (Recommendation: only if you actually hold a Copilot subscription you want spent.)
3. **#877 TDAI memory retention/pruning** — retention policy for the gateway SQLite store. (Recommendation: 90-day default with project-scoped exemption for pinned sessions; I can implement once the policy is named.)
4. **#918 project context import** — "resurrect an old project" scope. (Recommendation: park until you actually resurrect one; the import is a one-operator need today.)

## D. Already unblocked, ready for implementation

- **#149 routing policy editing** — its blockers (#532, #638) are now closed. Largest remaining Settings feature; needs the mutation design (config write path exists from #1175's work).
- **#503 typed block reasons/remediation** — partially shipped (approval gates, blockers lists); remainder is bounded.
- **#1176 openhands chat** — needs the openhands binary on a verifiable machine.
- **#936/#937/#939/#941/#942** — product surface work, all awaiting prioritization rather than decisions.

## Suggested next implementation stretch

1. Close #516/#517 after the verification pass (§A).
2. Fund #519's four JSON-ready read adapters + #525's manifest-generated schemas.
3. Implement #938's doc correction + #741 readiness on top.
4. Then #653's decision above determines the notifications path.
