# ACP integration evaluation — issue #1392

Evaluated 2026-10-06. Scope: backend integration, not a replacement for GAH's
dispatch or control plane. This document records a recommendation and future
verification gates; it does not claim those gates have already passed.

## Current usage and evidence

- [Manager chat registry](../apps/server/src/managerChat/registry.ts) selects the
  shared [TypeScript ACP client](../apps/server/src/managerChat/acpAdapter.ts)
  for Hermes and OpenCode's native `acp` commands, and Codex and Claude's bridge
  packages. Vibe, AGY, and OpenHands use the
  [headless adapter](../apps/server/src/managerChat/headlessAdapter.ts).
- The ACP client uses newline-delimited JSON-RPC over child-process stdio,
  initializes a connection, creates a session in the selected working directory,
  streams assistant and tool updates, discovers commands and configuration
  options, and sends cancellation. Model and reasoning selection require
  advertised configuration options. Permissions are declined or cancelled when
  no permission UI is attached. New sessions currently receive `mcpServers: []`.
- Connections are retained per profile within a backend. Reconnection uses a
  new session and transcript replay; this is not durable `session/load` resume.
  Steering has separate support detection and must not be assumed universal.
- [Rust manager sessions](../src/manager/mod.rs) already expose a provider-neutral
  lifecycle with explicit optional resume, interrupt, and inspect capabilities.
  [Hermes](../src/manager/hermes.rs) implements ACP directly, persists the
  GAH-to-provider session mapping, negotiates resume/load support, and declines
  permission requests. [Codex](../src/manager/codex.rs) uses its app-server
  protocol and [Claude](../src/manager/claude.rs) uses its native CLI surface.
  A shared GAH interface does not require every provider to use ACP.
- [Server package dependencies](../apps/server/package.json) declare ACP SDK
  `^1.4.0`, Claude bridge `^0.70.0`, and Codex bridge `^1.10.0`; the lockfile,
  rather than these ranges, determines installed versions. Bridge runtime
  requirements need independent validation: the
  [Linux standalone audit](validation/linux-standalone-audit-2026-10-03/README.md)
  records a Node-version compatibility concern.
- Existing evidence includes
  [ACP integration tests](../apps/server/src/acpAdapter.integration.test.ts),
  headless adapter tests, and Rust manager contract/provider tests. Fake-agent
  tests establish GAH behavior, not compatibility with every installed agent.

The upstream [protocol overview](https://agentclientprotocol.com/protocol/v1/overview)
describes initialization, prompt/update, cancellation and permission exchange.
The [session setup specification](https://agentclientprotocol.com/protocol/v1/session-setup)
requires checking `loadSession` before loading a prior session. These sources
were consulted on the evaluation date; supported optional features must still
be checked against GAH's locked SDK and the actual agent version.

## Candidate surfaces and cost/benefit

| Surface | Benefit | Cost and boundary | Recommendation |
| --- | --- | --- | --- |
| Interactive manager chat | Shared streaming, tool activity, permission UI, commands and model configuration across agents; already implemented | Bridge packaging, lifecycle differences and optional-feature drift remain provider-specific | Retain existing ACP integrations; prioritize compatibility evidence |
| Durable manager reconnect | Native session loading could avoid transcript replay and preserve provider context | Requires advertised support, persisted identity, history replay deduplication and restart tests; provider sessions are not interchangeable | Consider a bounded follow-up after compatibility inventory |
| Rust manager adapters | ACP can fit behind `ManagerSession` where a native agent supports it | Rewriting Codex/Claude loses native behavior unless parity is proven; parallel TS/Rust implementations increase maintenance | Keep provider-neutral interface and existing native adapters |
| Worker execution and continuation | Structured progress and cancellation could reduce provider output parsing | Must preserve worktree isolation, attempt identity, artifacts, exit/failure semantics, quota signals and dispatch recovery; prompt completion alone proves no ticket acceptance | Not yet; require an opt-in single-backend experiment and evidence first |
| GAH API, dashboard and MCP tools | Agent sessions can consume GAH tools through MCP if deliberately configured | ACP does not replace authenticated APIs, tool schemas, control-plane authorization or event cursors | Keep existing contracts; assess MCP exposure separately |
| Quota, cost and execution identity | Structured usage can supplement existing telemetry | Usage/config support varies; selected model does not prove actual model, and ACP does not establish subscription quota or billing class | Use only observed fields; retain existing telemetry sources |

## Risks and required boundaries

1. **Capability and version drift:** SDK, bridge and native agent releases can
   disagree. Record executable/version, negotiated capabilities and tested
   package versions. Unsupported features remain explicit, not silent success.
2. **Persistence and outcome:** keep runtime state and attempts in the ledger
   and session artifacts. Preserve GAH IDs separately from provider session IDs.
   Test child exit, malformed output, pending permission cancellation and
   reconnect. ACP stop reasons cannot replace tests, review or delivery evidence.
3. **Permissions and isolation:** an ACP permission choice authorizes a tool
   request, not GAH publication, merge or provider writes. Keep GAH policy checks
   authoritative. Do not advertise client filesystem/terminal capabilities
   without validating worktree boundaries and resource cleanup. Stdio transport
   itself is not a sandbox; subprocess credentials and environment still matter.
4. **Telemetry and identity:** absent usage, cost, quota, model and outcome stay
   unknown. Backend instance, requested model, actual model and usage class stay
   separate. Context occupancy is not billed token usage. Avoid deriving actual
   model from a selected configuration option or zero cost from missing reports.
5. **Operational cost:** maintaining bridges adds runtime and upgrade work;
   keeping TS and Rust implementations creates duplicate compatibility work.
   No measured latency, token savings or maintenance savings are established by
   this evaluation. Shared protocol shape alone is insufficient migration evidence.

## Recommendation and sequenced plan

**Retain ACP for current interactive manager integrations. Not yet for broader
worker adoption or replacement of native Rust adapters, because durable resume,
permission policy, telemetry and dispatch recovery parity are not demonstrated
across those boundaries.** No new runtime adoption is made by this ticket.

1. **Inventory before implementation:** backend maintainers record locked SDK
   and bridge versions, supported Node runtime, installed agent versions, and
   capabilities for each existing ACP backend. Run fake-agent tests and a
   credentialed smoke check of new/prompt/cancel, tool updates, permission denial,
   advertised configuration and missing usage. Deliver a compatibility matrix;
   unavailable live evidence is a human-review outcome, not a pass.
2. **Harden the existing manager boundary:** address only demonstrated gaps from
   that matrix in separate scoped tickets. Verify process death, cancellation
   during permission requests, reconnect and absent telemetry. Keep unsupported
   controls unavailable and preserve native/headless fallbacks explicitly.
3. **Evaluate durable loading for one supporting backend:** require negotiated
   `loadSession`, durable GAH/provider ID mapping and restart/history replay
   tests without duplicate turns or usage. Compare with current transcript replay.
   Proceed only when the benefit and lifecycle compatibility are evidenced;
   otherwise retain replay and document the limitation.
4. **Revisit worker transport only after manager evidence:** trial one opt-in
   backend behind existing dispatch interfaces. Require unchanged ledger/artifact
   ownership, identity, worktree isolation, interruption/recovery, verification
   and approval behavior. Compare the same task against its native runner;
   rollback means disabling the experiment and retaining that runner. Do not
   silently transfer provider session IDs across backends or resume workers
   through manager chat.
5. **Decision gate before any expansion:** record an accepted ADR or explicit
   owner decision naming the surface, supported versions, compatibility evidence,
   fallback and rollback. Missing parity evidence means defer and human review.

## ADR #1392: ACP remains a bounded manager transport

Status: recorded decision for this evaluation; future expansion is deferred.

Context: ACP is already used by manager chat and Rust Hermes, while GAH owns
task dispatch, policy, verification, runtime state and backend-neutral identity.

Decision: retain the existing ACP manager integrations and their current scope;
do not introduce ACP worker execution, replace native manager transports, or
expose GAH itself as an ACP agent in this change. Future adoption requires the
sequenced evidence and decision gate above.

Consequences: current shared manager UX remains useful without a protocol-wide
migration. Native/headless adapters remain supported, and GAH continues to own
authoritative lifecycle and approval semantics. Compatibility maintenance is
still required for the existing ACP dependencies.

## Verification

This is a documentation-only change. Review the source links above against the
implementation and run the repository checks from `CONTRIBUTING.md`:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
npm run typecheck
npm run test:server
```

These offline suites do not constitute live provider compatibility evidence;
the staged plan explicitly requires that evidence before expanding adoption.
