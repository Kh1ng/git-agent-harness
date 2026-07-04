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
| `agy_quota_exhausted.txt` | Captured locally from AGY cli.log | Real AGY RESOURCE_EXHAUSTED (code 429) quota exhaustion message. |
| `agy_auth_failed.txt` | Captured locally from AGY cli.log | Real AGY authentication error ("You are not logged into Antigravity"). |
