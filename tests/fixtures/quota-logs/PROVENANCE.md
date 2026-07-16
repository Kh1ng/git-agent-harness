# Quota/rate-limit fixture provenance

TICKET-066. Every fixture in this directory is copied verbatim (message
text only — no invented surrounding log content) from a real issue report
in the official `openai/codex` or `anthropics/claude-code` repositories.
Do not add a fixture here without a real issue reference. Do not hand-edit
the message text in an existing fixture without updating this file.

Captured into this repo: 2026-07-04.

| Fixture | Source | Notes |
|---|---|---|
| `codex_usage_exhausted_full_reset.txt` | github.com/openai/codex issue #12299 | Full date + time, no timezone. |
| `codex_usage_exhausted_admin_variant.txt` | github.com/openai/codex issue #16906 | Business/admin wording variant, full date + time, no timezone. |
| `codex_usage_exhausted_time_only.txt` | github.com/openai/codex issue #16847 | Time-only reset, no date, no timezone. |
| `claude_usage_exhausted_tz_reset.txt` | github.com/anthropics/claude-code issue #9236 | Time-only reset with an explicit IANA timezone name. |
| `claude_weekly_limit_structured.json` | github.com/anthropics/claude-code issue #68816 | Structured event; weekly limit language, month+day+time with explicit IANA timezone name, no year. |
| `claude_generic_rate_limit.json` | github.com/anthropics/claude-code issues #41583, #33840 | Structured event; generic "Rate limit reached" with no reset information. Reports in these issues indicate this can be client-side/session-related rather than account quota exhaustion — treated conservatively (low confidence) by the parser. |
| `claude_generic_rate_limit_zero_tokens.json` | github.com/anthropics/claude-code issues #41583, #33840 | Same family as above; some captured examples report zero input/output tokens alongside the error. |
| `claude_transient_throttle.json` | github.com/anthropics/claude-code issue #64030 | Explicit server-side throttling, explicitly NOT account usage-limit exhaustion. |
| `claude_session_limit_tz_reset.txt` | Local capture, live session `aca7ce98-c75c-4e11-930a-7f769d10b01b` (redacted) | Exact shape from the live Claude session-limit cooldown incident. |
| `agy_auth_not_logged_in.txt` | Local capture, `/tmp/agy-debug.log`, 2026-07-04 | AGY/Antigravity has no public issue tracker to cite; captured directly from a real local `agy` process log during an unauthenticated run on this host. |
| `opencode_hy3_rate_limit.log` | Local capture, `~/.local/share/opencode/log/opencode.log`, 2026-07-12 | OpenCode's Hy3-free provider error is written to OpenCode's internal log rather than GAH's captured stdout/stderr. |
| `vibe_invalid_api_key.txt` | Local capture, `artifacts/gah/sessions/61dad4cf-8e9a-4eb1-a64e-1640890e13d2/review-stderr.log`, 2026-07-15 | Vibe 2.19.1 rejected the configured Mistral credential before model invocation. |

AGY quota-exhaustion text (`RESOURCE_EXHAUSTED`, `Individual quota reached`, `code 429`)
matched by `agy_quota_re` in `src/quota_parser.rs` predates this provenance file and has no
corresponding captured fixture on this host — no real AGY quota exhaustion has been observed
here yet. The quota-branch tests in `quota_parser.rs` exercise the literal strings already
present in the shipped regex rather than an external fixture; treat that regex as unverified
against a real capture until one exists.
