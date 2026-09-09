# Chartr map mechanism and a minimal GAH contract

Researched 2026-09-09 for [#793](https://github.com/Kh1ng/git-agent-harness/issues/793). Research and proposed contract only; no product decision or implementation.

Chartr derives a dependency graph from Markdown, marks its runnable frontier, and starts a ticket session with a freshly assembled brief. GAH can reuse those semantics without adopting chartr's tracker, process manager, or skill registry.

## Source scope and changes since the reference clone

The issue's central-node clone was clean at `1c0a47e5ed087d7ab8c8f093f3c60b87d60dae95`. Upstream HEAD was `be8073f6ffe771f034e6a6b4d44400621ba37f5b`, dated 2026-08-20, when checked. The histories diverge. This report uses current upstream for the contract and identifies differences from the designated clone. Neither application was launched. Sources and existing tests were read; those tests were not run. [Reference revision](https://github.com/rengwu/chartr/tree/1c0a47e5ed087d7ab8c8f093f3c60b87d60dae95), [upstream revision](https://github.com/rengwu/chartr/commit/be8073f6ffe771f034e6a6b4d44400621ba37f5b).

Three differences matter:

| Mechanism | Designated clone | Current upstream |
| --- | --- | --- |
| Discovery | Recursively finds `map.md` below `.plan/`, including flat layouts | Only immediate map directories under `.plan/maps/` |
| Skills | Built-in, user, then committed workspace overrides; glossary and skill manifest in context | Registered sources and qualified or `auto` role bindings; conventions pointer, preferences, selected prompt presets, and source manifest |
| Claim provenance | Writes a ticket claim commit | Edits ticket and appends `.plan/audit.jsonl`; performs no VCS command |

These are source changes, not interchangeable descriptions. [Old discovery](https://github.com/rengwu/chartr/blob/1c0a47e5ed087d7ab8c8f093f3c60b87d60dae95/internal/mapscan/mapscan.go), [current discovery][discovery], [old composition](https://github.com/rengwu/chartr/blob/1c0a47e5ed087d7ab8c8f093f3c60b87d60dae95/internal/prompt/compose.go), [current composition][compose], [old claim launch](https://github.com/rengwu/chartr/blob/1c0a47e5ed087d7ab8c8f093f3c60b87d60dae95/internal/server/spawn.go), [current claims][claims].

## File format

The canonical writer contract is `.plan/maps/<lower-kebab-slug>/map.md`, sibling `tickets/NN-slug.md`, optional `assets/`, and optional `spec.md`. Numbers are permanent identities within a map. Writers use `01` through `99`, then natural width. Planning and implementation maps share the format; `-impl` is an implementation-map naming convention. The renderer does not parse `spec.md`. [Write contract][format].

A minimal example, with one resolved premise and one open dependent:

````text
.plan/maps/node-handoff/
  map.md
  tickets/01-transfer-scope.md
  tickets/02-implement-transfer.md
````

```markdown
# Node handoff

## Destination
Move a project's committed branch to another node.

## Notes
Keep local edits on the original node.

## Decisions so far
- [Transfer scope](./tickets/01-transfer-scope.md): committed changes only.

## Not yet specified
- **Delivery.** Implement the transfer. <clears-with: 02>

## Out of scope
- Moving uncommitted files.
```

`01-transfer-scope.md`:

```markdown
---
type: grilling
blocked_by: []
---
# Transfer scope

## Question
Which changes move between nodes?

## Done when
The transfer boundary is explicit.

## Answer
Transfer committed changes. Keep local edits on the original node.
```

`02-implement-transfer.md`:

```markdown
---
type: task
blocked_by: [01]
undermined_by: []
assets: []
---
# Implement transfer

## Question
Implement transfer within the agreed boundary.

## Done when
The destination receives the committed branch; original local edits remain.
```

This illustrates syntax, not a new GAH decision. Recognized ticket fields are `type`, `blocked_by`, `undermined_by`, `assets`, and tool-owned `claimed_by`/`claimed_at`. Types are `grilling`, `research`, `prototype`, and `task`. `undermined_by` flags an answer for human review; it does not reopen it. Assets are relative to the map's assets directory. A stored `status` is ignored. [Write contract][format].

The reader accepts a deliberately small frontmatter subset: leading `---` delimiters, single-line keys and comma-separated inline lists. It is not a general YAML parser. Structural headings are exact and fenced examples do not change ticket status. Legacy loose headers remain readable. Lint reports malformed titles, types, edges, cycles, and inconsistent decision/fog indexes. Discovery keeps readable tickets and exposes defects rather than rejecting the entire map. [Parser][parser], [lint][lint], [discovery][discovery].

## Frontier and rendering

Status follows this precedence:

| File content | Derived status |
| --- | --- |
| Nonempty `## Answer` | `resolved` |
| Otherwise, nonempty `## Ruled out` | `out_of_scope` |
| Otherwise, nonempty `claimed_by` | `claimed` |
| Otherwise | `open` |

A bare closing heading does not close a ticket. Closure takes precedence over a leftover claim. For each open ticket, every blocker must exist and be **resolved**. Missing, open, claimed, and ruled-out blockers prevent admission. An empty blocker list passes. `## Proposed Answer` has no special meaning. The clone's tracker skill says blockers must be "closed", but its parser requires resolved, and current upstream's write contract corrects the wording. [Parser][parser], [frontier regression tests][parse-tests], [write contract][format].

```text
frontier = tickets sorted by number, filtered by:
  status(ticket) == open
  AND every blocked_by ID exists in this map
  AND status(each blocker) == resolved
```

An answer unblocks dependents as soon as the file changes. It proves neither test success nor merge. A nonempty map is marked finished when every parsed ticket is resolved or out of scope; this is separate from its lint diagnostics. [Discovery][discovery].

The TS layout uses ticket numbers and blocker-to-dependent edges only. It sorts numbers, seeds a PRNG with 1337, places stars on dependency-depth rings, and runs 420 relaxation steps. Repulsion, edge springs, and radial pull produce the constellation. A structural signature excludes status, so status-only updates preserve positions. Missing edges are omitted from drawing; diagnostics remain the place to explain them. The pairwise relaxation is quadratic in ticket count, so its performance on large GAH projects remains unmeasured. [Layout][layout].

The canvas renders five states: resolved, frontier, claimed, blocked, and out of scope. Session activity is a separate overlay. Svelte mounts the renderer, supplies snapshots, and receives ticket selection. Selection opens a detail pane; it does not immediately launch an agent. Ticket types preselect roles: grilling → grill, research → research, prototype → prototype, task → implement. The operator can choose another role. The pane offers a role and agent choice on frontier tickets and exposes a payload preview. Pan, zoom, fit, and remembered camera state belong to the renderer boundary. [Renderer][renderer], [wrapper][wrapper], [detail pane][detail], [visual states][theme].

Filesystem notices trigger server rebuilds; the control WebSocket sends a complete authoritative snapshot, including on reconnect. The browser replaces its model. The canvas needs only `mount`, `setModel`, selection, and disposal, so the same boundary fits GAH's React UI without importing Svelte. The React application is a proposed adaptation. [Watcher][watch], [control socket][control], [wrapper][wrapper].

## Ticket-to-session injection

Current upstream's launch sequence is:

1. Re-read the map and ticket. Reject a non-frontier ticket, invalid role, missing agent registration, or unavailable CLI. A second live session in one space requires the operator's concurrency override.
2. Refresh repo-local skill and convention copies. Resolve the chosen role skill. Compose core, role body, conventions pointer, operator preferences, selected presets, then source manifest, map body, ticket body, and direct blockers' answers.
3. Hash the exact Markdown payload. Stamp the ticket claim and append provenance to the map audit log. Save a private payload in `.chartr/run/<session-id>/payload.md` and archive another copy in chartr state.
4. Start the selected CLI in the space's PTY with one opening instruction to read that file. Return a session bound to that ticket. A failure after claiming leaves a claim for explicit recovery.

The preview uses the same composer, but preview and launch are separate reads. A later edit can change the launch payload. [Spawn][spawn], [preview][preview], [composition][compose], [claims][claims].

Only direct blocker answers are inlined, in `blocked_by` order. Corrections and amendments immediately following an answer travel with it. Missing answers appear as explicit missing-context text in previews. The full dependency tree, asset bytes, and previous terminal transcript are not automatically included. Current upstream does not inject the clone's standalone glossary block. [Blocker assembly][preview], [answer extraction][compose].

The brief is **submitted**, not left in an input buffer. Delivery uses a trailing argument, a configured prompt flag, or typed input. The typing fallback waits for PTY readiness, writes the opener, then sends a separate carriage return. This avoids treating Enter as pasted text. File access is still necessary: sending an opener does not prove the agent read the brief. Payloads use owner-only file permissions. [Adapter][adapter], [terminal readiness][delivery], [prompt submission][submission], [file permissions][modes].

Current skills come from registered sources. Before launch, chartr copies them into gitignored, repo-local paths so sandboxed agents can read supporting files. This mirror is mutable and globally sourced; it is not a portable, immutable session archive. A source commit and payload hash record different evidence. GAH should preserve that distinction if it adopts the pattern. [Skill mirror ADR][mirror], [payload provenance][prompt].

## Proposed minimum contract for GAH

This is a recommendation for later implementation review, not owner approval of #799.

- Give the server one map snapshot containing stable project/map/ticket identities, content revision, ticket bodies, dependency IDs, derived status, frontier, and diagnostics. A TS view renders that snapshot; it never decides claim ownership.
- Keep chartr-compatible files as an optional input format. Defer the authoritative storage choice to #799. For hosted issues, retain forge host, repository identity, and issue number; a bare `#12` is insufficient across GitHub and custom GitLab hosts.
- Make frontier computation a pure function over the snapshot. Preserve the resolved-versus-ruled-out distinction. For implementation work, connect eligibility to GAH's existing completion policy; Markdown prose must not bypass validation, CI, or merge gates.
- Submit a stable ticket identity and intended node/provider through existing GAH dispatch. Revalidate eligibility and acquire the existing claim/lease before work starts. A successful UI click alone cannot grant a claim.
- Assemble one inspectable brief from the current ticket, map orientation, and direct premise answers. Record the selected revisions, provider, node, and exact submitted payload hash. Preserve source labels through GAH's context budget reduction and expose any omitted premise.
- Keep payload delivery behind the existing backend runner. A remote worker must receive readable local files or equivalent submitted content. Do not send a Mac path to a Windows/WSL worker or copy a previous session's machine paths. Preserve the user's committed-changes-only transfer boundary.
- Reuse the stable-layout property and a small renderer boundary. Provide a textual dependency/frontier view alongside a graph for mobile and keyboard access. Select a ticket before showing its start action.

GAH already has backend/job-kind identities, central claim leases, node observations, and recorded context-budget handling. Those are the integration points, rather than a second scheduler or transcript store. [Backend runner](../src/runner/backend_runner.rs), [job kinds](../src/job_kind.rs), [central claims](../src/central_claims.rs), [registry service](../apps/server/src/registryService.ts), [context construction](../src/dispatch/prompts.rs).

The eventual acceptance checks should cover the four-state precedence, ruled-out/missing blockers, cycles, and fenced headings; unchanged coordinates after status updates; fresh blocker corrections in payloads; concurrent claim attempts; and readable, submitted context on the chosen worker. Neither this research nor chartr's unit tests establish iPhone usability or GAH's cross-node delivery.

## Decisions still open

[#799](https://github.com/Kh1ng/git-agent-harness/issues/799) still asks whether maps live in repository files, hosted issues, config, or the ledger. This research does not choose that authority, synchronization behavior, or ownership of map edits.

[#1077](https://github.com/Kh1ng/git-agent-harness/issues/1077) asks for per-ticket and per-project progress from durable session outcomes and settle events. That is a lifecycle view, while chartr's star map is a dependency view. The owner has not selected the combined interaction design. Researching the latter does not satisfy the former's acceptance criteria.

The [owner's obsolescence note](https://github.com/Kh1ng/git-agent-harness/issues/793#issuecomment-5400592849) reserves closure verification. Node/worker, provider, and device-split choices should not be reopened by this document. Leave #793 open for that verification; this artifact answers its mechanism/spec question and can inform the remaining map-format and visualization decisions.

[format]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/prompt/assets/conventions.md
[parser]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/wayfinder/parse.go
[parse-tests]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/wayfinder/wayfinder_test.go
[lint]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/wayfinder/lint.go
[discovery]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/mapscan/mapscan.go
[layout]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/web/src/lib/starmap/layout.ts
[renderer]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/web/src/lib/starmap/starmap.ts
[wrapper]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/web/src/lib/StarMap.svelte
[detail]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/web/src/lib/DetailPane.svelte
[theme]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/web/src/lib/starmap/theme.ts
[watch]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/server/watch.go
[control]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/server/control.go
[spawn]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/server/spawn.go
[preview]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/server/payload.go
[compose]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/prompt/compose.go
[claims]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/server/claim.go
[adapter]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/adapter/adapter.go
[delivery]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/terminal/manager.go
[modes]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/server/filemodes.go
[mirror]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/docs/adr/0018-skill-mirror-and-no-seed.md
[prompt]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/prompt/prompt.go
[submission]: https://github.com/rengwu/chartr/blob/be8073f6ffe771f034e6a6b4d44400621ba37f5b/internal/terminal/liveprompt.go
