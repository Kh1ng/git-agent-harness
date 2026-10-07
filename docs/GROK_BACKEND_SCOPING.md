# Grok backend scoping — #1232

Investigated 2026-10-07. This is the parked ticket's pre-unparking scoping
record, not evidence that Grok dispatch or Manager Chat works in GAH.

## Evidence and decision

An official command-line agent exists: **Grok Build**. The
[official overview](https://docs.x.ai/build/overview) links an installer at
`https://x.ai/cli/install.sh` and describes browser authentication or
`XAI_API_KEY`. Third-party projects named grok-cli are not this agent.

Neither `command -v grok` nor `command -v opencode` found an executable in
the assigned environment. Fetching the official installer with
`curl -fSL --connect-timeout 10 --max-time 30 https://x.ai/cli/install.sh`
failed with exit 6 (`Could not resolve host: x.ai`). Consequently, the
installed version, real help, auth behavior, output schema, usage reporting,
and authenticated smoke remain **unverified**. Web documentation does not
satisfy the ticket's real `--help` requirement.

The following are documentation leads to check against a pinned executable,
not a verified invocation contract:

| Capability | Officially documented lead |
| --- | --- |
| Headless prompt | `-p, --single <PROMPT>` |
| Output | `--output-format plain`, `json`, or `streaming-json` |
| Model | `-m, --model <MODEL>` |
| Working directory | `--cwd <PATH>` |
| Resume | `--resume`, `--continue`, `--session-id` |
| Authentication | `grok login`, `grok login --device-auth`, or `XAI_API_KEY` |
| Chat transport | `grok agent stdio` (ACP) |
| Automated execution | `--no-auto-update` |

Sources: [CLI reference](https://docs.x.ai/build/cli/reference) and
[headless/ACP documentation](https://docs.x.ai/build/cli/headless-scripting).
The reference describes optional resume IDs while the headless page lists an
explicit ID; resolve this with real help before implementing resume.

**Decision: keep #1232 parked pending executable and authenticated evidence.**
A native runner plus ACP chat adapter is a plausible future scope, but suitability
is not established. Do not add a speculative runner or mark Grok available.
The [OpenCode provider documentation](https://opencode.ai/docs/providers/)
also lists xAI authentication via `/connect` and model selection via `/models`.
Routing `xai/<verified-model-id>` through the existing `opencode` runner is an
alternative requiring config/docs rather than a new runner. That path still
needs real OpenCode help, model enumeration, auth-failure evidence and both
authenticated smokes; it has not been selected or verified here.

## Existing compatibility boundaries

- `packages/contracts/src/ws.ts` already includes `grok` in `ProviderKind`;
  this is not implementation evidence.
- `src/status.rs` deliberately omits unimplemented Grok from backend probes.
- `src/runner/backends/opencode.rs` implements OpenCode dispatch;
  `apps/server/src/managerChat/registry.ts` and `acpAdapter.ts` implement
  OpenCode chat. These are reuse points for the alternative path.
- Keep backend instance, logical backend, requested model, actual model and
  usage class separate. Missing usage/cost must remain unknown.
- Auth failure must surface as backend unavailable with reason `auth` at the
  consumer boundary; test compatibility with internal `auth_failure` records.

## Local follow-up drafts

These drafts are reviewable split scopes, not published issues. No provider
writes or issue-status changes were made.

### Runner: Grok Build headless dispatch

Prerequisite: capture `grok version`, `grok --help`, `grok login --help` and
`grok agent --help` from a pinned official install; retain stdout, stderr and
exit codes in session artifacts. Confirm the flags above and permission/tool
controls without performing authentication during help discovery.

Scope: add the verified runner to backend resolution, config and status probes;
reuse supervision, timeout and artifact capture. Preserve selected instance and
requested/actual model identities in ledger records. Parse only observed usage;
absent counters or cost remain unknown. Classify authentication failures.

Acceptance: a research dispatch with the selected Grok backend completes;
artifacts and ledger prove identity, model and usage semantics. A missing or
invalid credential produces backend-unavailable reason `auth`. Fake CLI tests
cover argv, cwd, failure, timeout and absent usage. A real authenticated research
smoke is required before merge, with version and sanitized evidence retained.

Verification: `cargo fmt --check`; focused runner/auth/usage tests; `cargo test`;
`cargo clippy --all-targets --all-features -- -D warnings`. Use
`gah dispatch --help` to determine the complete research smoke command for the
selected profile; do not invent a dispatch invocation from docs alone.

### Chat: Grok Build ACP Manager Chat

Prerequisite: runner discovery evidence plus real `grok agent stdio --help`;
observe ACP initialize/auth/session capabilities with the pinned executable.

Scope: reuse the ACP adapter and instance launch boundaries, register Grok only
when implemented, and support model selection and per-turn attribution. Resume
only when the observed ACP capability permits it; CLI resume flags alone are
not ACP resume evidence. Keep unknown usage unknown.

Acceptance: Manager Chat streams an authenticated Grok reply with per-turn
backend instance and requested/actual model attribution. Auth failure produces
backend-unavailable reason `auth`. Adapter tests cover incremental chunks,
model selection, cancellation, capability-dependent resume and missing usage.
Retain a real authenticated Manager Chat smoke before merge.

Verification: `npm run test:server`; `npm run typecheck`; `npm run build:server`;
`npm run build:web`; authenticated Manager Chat smoke using the selected instance.

If OpenCode is selected instead, replace these native implementation scopes with
an xAI instance/config/docs scope and retain the same dispatch, chat, auth,
identity and authenticated-evidence acceptance requirements.
