//! TICKET-066: parse rate-limit, context-limit, and quota-reset signals from
//! backend failure output.
//!
//! Every classification pattern in this module exists because a real,
//! provenance-tracked example justified it — see
//! `tests/fixtures/quota-logs/PROVENANCE.md` and the fixture-driven tests
//! in this module's own `#[cfg(test)]` block below (this is a binary
//! crate with no library target, so integration tests under `tests/`
//! cannot call into `src/` modules directly — fixture tests for pure
//! `src/` logic live as unit tests here instead, loading the same fixture
//! files via `include_str!`). Do not add a new pattern here without a
//! matching real fixture. No live provider calls are made or required —
//! this only ever parses text already captured elsewhere.
//!
//! Not yet wired into runner/routing (TICKET-067/068) — this module is a
//! pure function over text, complete and tested on its own.
#![allow(dead_code)]

use regex::Regex;
use std::sync::OnceLock;
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Account/subscription usage or weekly limit exhausted. Not worth
    /// retrying until (if known) `reset_at`.
    QuotaExhausted,
    /// Transient throttling (HTTP 429-shaped, server load, or an
    /// unspecified "rate limit" with no indication it's account-level
    /// exhaustion). Worth retrying after a short cooldown.
    RateLimited,
    /// Authentication failure (invalid token, not logged in, etc).
    AuthenticationError,
    /// GAH observed a genuinely idle backend process. This is a harness
    /// watchdog classification, never provider quota/rate-limit evidence.
    BackendStalled,
    /// Explicit model context-window/context-length exhaustion.
    ContextLimitExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Recognized a generic/ambiguous pattern with real-world reports of
    /// it meaning different things in different contexts (see the generic
    /// Claude "Rate limit reached" fixture's provenance note).
    Low,
    Medium,
    /// Language is unambiguous about which failure kind this is.
    High,
}

#[derive(Debug, Clone)]
pub struct ParsedFailure {
    pub backend: String,
    pub kind: FailureKind,
    /// Whether a short-term retry loop is worth attempting at all. `false`
    /// for quota/billing exhaustion; `true` for transient rate limiting or a
    /// bounded reroute after a harness-observed backend stall.
    pub retryable: bool,
    /// RFC3339. Only set when resolved with actual confidence — see
    /// `unresolved_timezone` for the case where we recognized a reset
    /// time but couldn't resolve it to an absolute instant.
    pub reset_at: Option<String>,
    pub retry_after_seconds: Option<u64>,
    pub confidence: Confidence,
    /// The specific substring that triggered classification, for audit
    /// trails and debugging false positives/negatives.
    pub matched_evidence: String,
    /// Set when the message named an explicit IANA-style timezone (e.g.
    /// "America/Santiago") that this module cannot resolve to an absolute
    /// instant without a timezone database — a dependency deliberately
    /// not added for this ticket. `reset_at` is left `None` in that case
    /// rather than guessing an offset.
    pub unresolved_timezone: Option<String>,
}

/// A short, conservative cooldown for rate-limit classifications that
/// carry no better timing information from the provider. This is a
/// policy default, not something read from the message — never applied
/// to quota_exhausted, where a made-up short cooldown would be actively
/// harmful (hammering an exhausted account).
const CONSERVATIVE_RATE_LIMIT_COOLDOWN_SECONDS: u64 = 60;

/// A bounded, model-specific cooldown recorded when a backend reports a
/// context-window/context-length exhaustion. Unlike quota/auth exhaustion it
/// is NOT an account- or pool-wide condition, and it must never be permanent:
/// a single oversized task must not take the model (or, via a shared
/// quota_pool, every candidate) out of rotation indefinitely.
const CONTEXT_LIMIT_COOLDOWN_SECONDS: u64 = 600;

fn transient_not_usage_limit_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)not your usage limit").unwrap())
}

fn usage_or_weekly_limit_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(usage limit|weekly limit)").unwrap())
}

fn insufficient_balance_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(account balance (?:is )?insufficient|insufficient (?:account )?balance|credit balance.{0,24}(?:insufficient|exhausted)|payment required)",
        )
        .unwrap()
    })
}

fn context_limit_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // The exhaustion verb alternation deliberately does NOT include a bare
        // `too`: a generic "too verbose"/"too slow" far from any limit word can
        // straddle the 80-char windows and falsely match against an unrelated
        // "token limit" mention. Only the explicit `too long` / `too many`
        // phrases count. This keeps ordinary backend chatter from being
        // misclassified as a context-window exhaustion (which would otherwise
        // disable a model/pool — see availability.rs).
        Regex::new(
            r"(?i)(?:(?:\b(?:context|token|prompt|input)\b.{0,80}(?:window|length|limit|budget|size)|(?:context|token|prompt|input)\s+length).{0,80}(?:exceeded|exceeding|over|beyond|too long|too many|surpassed)|(?:exceeded|exceeding|over|beyond|too long|too many|surpassed).{0,80}\b(?:context|token|prompt|input)\b.{0,80}(?:window|length|limit|budget|size))",
        )
        .unwrap()
    })
}

fn generic_rate_limit_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)rate limit").unwrap())
}

/// Shape: "Feb 23rd, 2026 9:01 PM" / "Apr 7th, 2026 1:07 AM" — full date
/// and time, no timezone.
fn full_date_time_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b([A-Za-z]{3,9})\.?\s+(\d{1,2})(?:st|nd|rd|th)?,\s*(\d{4})\s+(\d{1,2}):(\d{2})\s*(AM|PM)\b",
        )
        .unwrap()
    })
}

/// Shape: "2:57 PM" — time only, no date, no timezone.
fn time_only_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bat\s+(\d{1,2}):(\d{2})\s*(AM|PM)\b").unwrap())
}

/// Shape: "3pm (America/Santiago)" — hour only (no minutes), explicit
/// named timezone in parens.
fn time_only_with_tz_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(\d{1,2})\s*(am|pm)\s*\(([A-Za-z_]+/[A-Za-z_]+)\)").unwrap()
    })
}

/// Shape: "Jun 3 at 4pm (Europe/Berlin)" — month + day (no year), explicit
/// named timezone in parens.
fn month_day_time_with_tz_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b([A-Za-z]{3,9})\s+(\d{1,2})\s+at\s+(\d{1,2})\s*(am|pm)\s*\(([A-Za-z_]+/[A-Za-z_]+)\)",
        )
        .unwrap()
    })
}

fn agy_quota_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(RESOURCE_EXHAUSTED|Individual quota reached|code 429|AGY quota exhausted)",
        )
        .unwrap()
    })
}

fn agy_auth_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(not logged into Antigravity|not logged in|AGY not authenticated)")
            .unwrap()
    })
}

fn parse_agy_cooldown_seconds(text: &str) -> Option<u64> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)\bResets\s+(?:in|~)\s*(?:(\d+)h)?\s*(?:(\d+)m)?\s*(?:(\d+)s)?\b").unwrap()
    });
    if let Some(caps) = re.captures(text) {
        let mut total_secs = 0u64;
        let mut matched = false;
        if let Some(h_match) = caps.get(1) {
            if let Ok(h) = h_match.as_str().parse::<u64>() {
                total_secs += h * 3600;
                matched = true;
            }
        }
        if let Some(m_match) = caps.get(2) {
            if let Ok(m) = m_match.as_str().parse::<u64>() {
                total_secs += m * 60;
                matched = true;
            }
        }
        if let Some(s_match) = caps.get(3) {
            if let Ok(s) = s_match.as_str().parse::<u64>() {
                total_secs += s;
                matched = true;
            }
        }
        if matched && total_secs > 0 {
            return Some(total_secs);
        }
    }
    None
}

fn parse_month(name: &str) -> Option<Month> {
    let lower = name.to_ascii_lowercase();
    let key = &lower[..lower.len().min(3)];
    Some(match key {
        "jan" => Month::January,
        "feb" => Month::February,
        "mar" => Month::March,
        "apr" => Month::April,
        "may" => Month::May,
        "jun" => Month::June,
        "jul" => Month::July,
        "aug" => Month::August,
        "sep" => Month::September,
        "oct" => Month::October,
        "nov" => Month::November,
        "dec" => Month::December,
        _ => return None,
    })
}

fn to_24h(hour12: u8, ampm: &str) -> u8 {
    let h = hour12 % 12;
    if ampm.eq_ignore_ascii_case("pm") {
        h + 12
    } else {
        h
    }
}

/// Reset-time resolution result: `(reset_at_rfc3339, unresolved_timezone)`.
/// At most one of the two is ever `Some` for a given input.
fn extract_reset(text: &str, now: OffsetDateTime) -> (Option<String>, Option<String>) {
    // Named-timezone shapes take priority and are checked first: if a
    // timezone is explicitly present, never fall through to a no-timezone
    // shape that would silently assume `now`'s offset instead.
    if let Some(caps) = month_day_time_with_tz_re().captures(text) {
        return (None, Some(caps[5].to_string()));
    }
    if let Some(caps) = time_only_with_tz_re().captures(text) {
        return (None, Some(caps[3].to_string()));
    }

    if let Some(caps) = full_date_time_re().captures(text) {
        if let (Some(month), Ok(day), Ok(year), Ok(hour12), Ok(minute)) = (
            parse_month(&caps[1]),
            caps[2].parse::<u8>(),
            caps[3].parse::<i32>(),
            caps[4].parse::<u8>(),
            caps[5].parse::<u8>(),
        ) {
            let hour = to_24h(hour12, &caps[6]);
            if let (Ok(date), Ok(time)) = (
                Date::from_calendar_date(year, month, day),
                Time::from_hms(hour, minute, 0),
            ) {
                let naive = PrimitiveDateTime::new(date, time);
                let resolved = naive.assume_offset(now.offset());
                return (
                    resolved
                        .format(&time::format_description::well_known::Rfc3339)
                        .ok(),
                    None,
                );
            }
        }
        return (None, None);
    }

    if let Some(caps) = time_only_re().captures(text) {
        if let (Ok(hour12), Ok(minute)) = (caps[1].parse::<u8>(), caps[2].parse::<u8>()) {
            let hour = to_24h(hour12, &caps[3]);
            if let Ok(time) = Time::from_hms(hour, minute, 0) {
                let mut candidate =
                    PrimitiveDateTime::new(now.date(), time).assume_offset(now.offset());
                if candidate <= now {
                    candidate += time::Duration::days(1);
                }
                return (
                    candidate
                        .format(&time::format_description::well_known::Rfc3339)
                        .ok(),
                    None,
                );
            }
        }
        return (None, None);
    }

    (None, None)
}

/// Parse a single backend failure message. Returns `None` when nothing
/// recognizable as quota exhaustion or rate limiting is present — callers
/// must be able to tell "we saw this and don't know what it is" apart from
/// "we're confident this isn't a quota/rate-limit failure at all".
pub fn parse(backend: &str, text: &str, now: OffsetDateTime) -> Option<ParsedFailure> {
    // AGY-specific patterns checked first: auth errors and quota exhaustion.
    if matches!(backend, "agy" | "agy-main" | "agy-second") {
        if let Some(m) = agy_auth_re().find(text) {
            return Some(ParsedFailure {
                backend: backend.to_string(),
                kind: FailureKind::AuthenticationError,
                retryable: true,
                reset_at: None,
                retry_after_seconds: None,
                confidence: Confidence::High,
                matched_evidence: extract_evidence_line(text, m.start(), m.end()),
                unresolved_timezone: None,
            });
        }

        if let Some(m) = agy_quota_re().find(text) {
            let reset_at = parse_agy_cooldown_seconds(text).map(|secs| {
                let reset_time = now + time::Duration::seconds(secs as i64);
                reset_time
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default()
            });
            return Some(ParsedFailure {
                backend: backend.to_string(),
                kind: FailureKind::QuotaExhausted,
                retryable: false,
                reset_at,
                retry_after_seconds: None,
                confidence: Confidence::High,
                matched_evidence: extract_evidence_line(text, m.start(), m.end()),
                unresolved_timezone: None,
            });
        }
    }

    // Highest precedence: an explicit "not your usage limit" disclaimer
    // must never be classified as quota exhaustion, even though the phrase
    // itself contains the substring "usage limit".
    if let Some(m) = transient_not_usage_limit_re().find(text) {
        let _ = m;
        return Some(ParsedFailure {
            backend: backend.to_string(),
            kind: FailureKind::RateLimited,
            retryable: true,
            reset_at: None,
            retry_after_seconds: Some(CONSERVATIVE_RATE_LIMIT_COOLDOWN_SECONDS),
            confidence: Confidence::High,
            matched_evidence: extract_evidence_line(text, m.start(), m.end()),
            unresolved_timezone: None,
        });
    }

    if let Some(m) = context_limit_re().find(text) {
        return Some(ParsedFailure {
            backend: backend.to_string(),
            kind: FailureKind::ContextLimitExceeded,
            retryable: false,
            reset_at: None,
            retry_after_seconds: Some(CONTEXT_LIMIT_COOLDOWN_SECONDS),
            confidence: Confidence::High,
            matched_evidence: extract_evidence_line(text, m.start(), m.end()),
            unresolved_timezone: None,
        });
    }

    if let Some(m) = usage_or_weekly_limit_re().find(text) {
        let (reset_at, unresolved_timezone) = extract_reset(text, now);
        return Some(ParsedFailure {
            backend: backend.to_string(),
            kind: FailureKind::QuotaExhausted,
            retryable: false,
            reset_at,
            retry_after_seconds: None,
            confidence: Confidence::High,
            matched_evidence: extract_evidence_line(text, m.start(), m.end()),
            unresolved_timezone,
        });
    }

    if let Some(m) = insufficient_balance_re().find(text) {
        return Some(ParsedFailure {
            backend: backend.to_string(),
            kind: FailureKind::QuotaExhausted,
            retryable: false,
            reset_at: None,
            retry_after_seconds: None,
            confidence: Confidence::High,
            matched_evidence: extract_evidence_line(text, m.start(), m.end()),
            unresolved_timezone: None,
        });
    }

    if let Some(m) = generic_rate_limit_re().find(text) {
        // Real-world reports (see PROVENANCE.md) show this generic phrasing
        // can be client-side/session-related rather than account quota
        // exhaustion, so this is deliberately low-confidence and carries
        // no invented reset time.
        return Some(ParsedFailure {
            backend: backend.to_string(),
            kind: FailureKind::RateLimited,
            retryable: true,
            reset_at: None,
            retry_after_seconds: Some(CONSERVATIVE_RATE_LIMIT_COOLDOWN_SECONDS),
            confidence: Confidence::Low,
            matched_evidence: extract_evidence_line(text, m.start(), m.end()),
            unresolved_timezone: None,
        });
    }

    None
}

/// The line containing the match, trimmed, for readable evidence in
/// multi-line log output; falls back to the raw match text for single-line
/// or JSON-blob inputs where "line" isn't a meaningful unit.
fn extract_evidence_line(text: &str, match_start: usize, match_end: usize) -> String {
    let line_start = text[..match_start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = text[match_end..]
        .find('\n')
        .map(|i| match_end + i)
        .unwrap_or(text.len());
    let line = text[line_start..line_end].trim();
    if line.is_empty() {
        text[match_start..match_end].to_string()
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    //! Every test here loads a real, provenance-tracked fixture from
    //! tests/fixtures/quota-logs/ (see PROVENANCE.md there) rather than an
    //! invented string. The only hand-written strings in this module are
    //! the negative-test inputs, which exist specifically to prove the
    //! parser does NOT match things it shouldn't — see
    //! `unrelated_text_containing_limit_does_not_match`.
    use super::*;

    fn utc(year: i32, month: Month, day: u8, hour: u8, minute: u8) -> OffsetDateTime {
        PrimitiveDateTime::new(
            Date::from_calendar_date(year, month, day).unwrap(),
            Time::from_hms(hour, minute, 0).unwrap(),
        )
        .assume_utc()
    }

    const CODEX_FULL_RESET: &str =
        include_str!("../tests/fixtures/quota-logs/codex_usage_exhausted_full_reset.txt");
    const CODEX_ADMIN_VARIANT: &str =
        include_str!("../tests/fixtures/quota-logs/codex_usage_exhausted_admin_variant.txt");
    const CODEX_TIME_ONLY: &str =
        include_str!("../tests/fixtures/quota-logs/codex_usage_exhausted_time_only.txt");
    const CLAUDE_TZ_RESET: &str =
        include_str!("../tests/fixtures/quota-logs/claude_usage_exhausted_tz_reset.txt");
    const CLAUDE_WEEKLY_LIMIT: &str =
        include_str!("../tests/fixtures/quota-logs/claude_weekly_limit_structured.json");
    const CLAUDE_GENERIC_RATE_LIMIT: &str =
        include_str!("../tests/fixtures/quota-logs/claude_generic_rate_limit.json");
    const CLAUDE_GENERIC_RATE_LIMIT_ZERO_TOKENS: &str =
        include_str!("../tests/fixtures/quota-logs/claude_generic_rate_limit_zero_tokens.json");
    const CLAUDE_TRANSIENT_THROTTLE: &str =
        include_str!("../tests/fixtures/quota-logs/claude_transient_throttle.json");
    const AGY_AUTH_NOT_LOGGED_IN: &str =
        include_str!("../tests/fixtures/quota-logs/agy_auth_not_logged_in.txt");
    const OPENCODE_HY3_RATE_LIMIT: &str =
        include_str!("../tests/fixtures/quota-logs/opencode_hy3_rate_limit.log");
    const CODEX_CONTEXT_LIMIT: &str =
        include_str!("../tests/fixtures/quota-logs/codex_context_limit_exceeded.txt");

    // ── Codex: full date+time, no timezone (openai/codex #12299) ───────────

    #[test]
    fn codex_full_reset_resolves_to_absolute_instant() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("codex", CODEX_FULL_RESET, now).unwrap();
        assert_eq!(parsed.kind, FailureKind::QuotaExhausted);
        assert!(!parsed.retryable);
        assert_eq!(parsed.confidence, Confidence::High);
        assert_eq!(parsed.reset_at.as_deref(), Some("2026-02-23T21:01:00Z"));
        assert_eq!(parsed.unresolved_timezone, None);
        assert!(parsed
            .matched_evidence
            .to_lowercase()
            .contains("usage limit"));
    }

    // ── Codex: business/admin variant (openai/codex #16906) ────────────────

    #[test]
    fn codex_admin_variant_full_reset_resolves() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("codex", CODEX_ADMIN_VARIANT, now).unwrap();
        assert_eq!(parsed.kind, FailureKind::QuotaExhausted);
        assert_eq!(parsed.reset_at.as_deref(), Some("2026-04-07T01:07:00Z"));
    }

    // ── Codex: time-only reset (openai/codex #16847) — day rollover ────────

    #[test]
    fn codex_time_only_reset_stays_same_day_when_still_ahead() {
        // "try again at 2:57 PM", observed at 10:00 UTC the same day.
        let now = utc(2026, Month::March, 10, 10, 0);
        let parsed = parse("codex", CODEX_TIME_ONLY, now).unwrap();
        assert_eq!(parsed.kind, FailureKind::QuotaExhausted);
        assert_eq!(parsed.reset_at.as_deref(), Some("2026-03-10T14:57:00Z"));
    }

    #[test]
    fn codex_time_only_reset_rolls_over_to_next_day_when_already_past() {
        // Same message, but observed at 18:00 UTC -- 2:57 PM has already
        // passed today, so the reset must roll over to tomorrow.
        let now = utc(2026, Month::March, 10, 18, 0);
        let parsed = parse("codex", CODEX_TIME_ONLY, now).unwrap();
        assert_eq!(parsed.reset_at.as_deref(), Some("2026-03-11T14:57:00Z"));
    }

    #[test]
    fn codex_time_only_reset_rolls_over_across_a_month_boundary() {
        // Exercise rollover specifically across a day/month boundary, not
        // just an ordinary day increment.
        let now = utc(2026, Month::March, 31, 18, 0);
        let parsed = parse("codex", CODEX_TIME_ONLY, now).unwrap();
        assert_eq!(parsed.reset_at.as_deref(), Some("2026-04-01T14:57:00Z"));
    }

    #[test]
    fn codex_context_limit_exceeded_is_distinct_from_quota_exhaustion() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("codex", CODEX_CONTEXT_LIMIT, now).unwrap();
        assert_eq!(parsed.kind, FailureKind::ContextLimitExceeded);
        assert_eq!(parsed.confidence, Confidence::High);
        assert!(!parsed.retryable);
        assert_eq!(parsed.reset_at, None);
        assert_eq!(
            parsed.retry_after_seconds,
            Some(CONTEXT_LIMIT_COOLDOWN_SECONDS),
            "context limit must have a bounded cooldown to avoid permanent backend disablement"
        );
    }

    // ── Claude: explicit IANA timezone (anthropics/claude-code #9236) ──────

    #[test]
    fn claude_tz_reset_classifies_quota_exhausted_but_leaves_reset_at_unset() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("claude", CLAUDE_TZ_RESET, now).unwrap();
        assert_eq!(parsed.kind, FailureKind::QuotaExhausted);
        assert_eq!(parsed.confidence, Confidence::High);
        assert_eq!(
            parsed.reset_at, None,
            "must not silently assume local timezone when an explicit zone is present"
        );
        assert_eq!(
            parsed.unresolved_timezone.as_deref(),
            Some("America/Santiago")
        );
    }

    // ── Claude: weekly limit, structured event (anthropics/claude-code #68816) ─

    #[test]
    fn claude_weekly_limit_structured_classifies_quota_exhausted_reset_unresolved() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("claude", CLAUDE_WEEKLY_LIMIT, now).unwrap();
        assert_eq!(parsed.kind, FailureKind::QuotaExhausted);
        assert_eq!(parsed.reset_at, None);
        assert_eq!(parsed.unresolved_timezone.as_deref(), Some("Europe/Berlin"));
    }

    // ── Claude: generic ambiguous rate_limit (anthropics/claude-code #41583, #33840) ─

    #[test]
    fn claude_generic_rate_limit_is_low_confidence_with_no_fabricated_reset() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("claude", CLAUDE_GENERIC_RATE_LIMIT, now).unwrap();
        assert_eq!(parsed.kind, FailureKind::RateLimited);
        assert_eq!(
            parsed.confidence,
            Confidence::Low,
            "reports show this generic phrasing can be client-side/session-related, not account quota exhaustion"
        );
        assert_eq!(parsed.reset_at, None);
        assert_eq!(
            parsed.retry_after_seconds,
            Some(CONSERVATIVE_RATE_LIMIT_COOLDOWN_SECONDS)
        );
    }

    #[test]
    fn claude_generic_rate_limit_zero_tokens_variant_classifies_the_same() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("claude", CLAUDE_GENERIC_RATE_LIMIT_ZERO_TOKENS, now).unwrap();
        assert_eq!(parsed.kind, FailureKind::RateLimited);
        assert_eq!(parsed.confidence, Confidence::Low);
        assert_eq!(parsed.reset_at, None);
    }

    // ── Claude: explicit transient throttle (anthropics/claude-code #64030) ─
    // Required negative test: this must never be classified as quota
    // exhaustion, even though its own text contains the substring
    // "usage limit" (inside "not your usage limit").

    #[test]
    fn claude_transient_server_throttle_is_not_quota_exhaustion() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("claude", CLAUDE_TRANSIENT_THROTTLE, now).unwrap();
        assert_eq!(parsed.kind, FailureKind::RateLimited);
        assert_ne!(parsed.kind, FailureKind::QuotaExhausted);
        assert!(parsed.retryable);
        assert_eq!(parsed.reset_at, None);
    }

    #[test]
    fn claude_context_window_exceeded_is_classified_as_context_limit() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("claude", "Context window exceeded (max 200000 tokens)", now).unwrap();
        assert_eq!(parsed.kind, FailureKind::ContextLimitExceeded);
    }

    #[test]
    fn opencode_internal_hy3_rate_limit_is_classified_without_a_fabricated_reset() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("opencode", OPENCODE_HY3_RATE_LIMIT, now).unwrap();
        assert_eq!(parsed.kind, FailureKind::RateLimited);
        assert_eq!(parsed.reset_at, None);
        assert_eq!(
            parsed.retry_after_seconds,
            Some(CONSERVATIVE_RATE_LIMIT_COOLDOWN_SECONDS)
        );
        assert!(parsed.matched_evidence.contains("Rate limit exceeded"));
    }

    #[test]
    fn opencode_insufficient_balance_is_non_retryable_quota_unavailability() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse(
            "opencode",
            "Error: Forbidden: Sorry, your account balance is insufficient",
            now,
        )
        .unwrap();
        assert_eq!(parsed.kind, FailureKind::QuotaExhausted);
        assert!(!parsed.retryable);
        assert_eq!(parsed.confidence, Confidence::High);
        assert_eq!(parsed.reset_at, None);
        assert!(parsed.matched_evidence.contains("balance is insufficient"));
    }

    #[test]
    fn opencode_input_length_exceeded_is_classified_as_context_limit() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse(
            "opencode",
            "error while batching request: prompt input length exceeded supported context length",
            now,
        )
        .unwrap();
        assert_eq!(parsed.kind, FailureKind::ContextLimitExceeded);
    }

    #[test]
    fn vibe_context_exceeded_is_classified_as_context_limit() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("vibe", "vibe request failed: input length exceeded", now).unwrap();
        assert_eq!(parsed.kind, FailureKind::ContextLimitExceeded);
    }

    #[test]
    fn agy_context_exceeded_is_classified_as_context_limit() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse(
            "agy",
            "AGY request rejected: context window length exceeded",
            now,
        )
        .unwrap();
        assert_eq!(parsed.kind, FailureKind::ContextLimitExceeded);
    }

    // ── Required negative tests ──────────────────────────────────────────

    #[test]
    fn generic_rate_limit_reached_does_not_produce_a_fabricated_reset() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("claude", CLAUDE_GENERIC_RATE_LIMIT, now).unwrap();
        assert_eq!(parsed.reset_at, None);
        assert_eq!(parsed.unresolved_timezone, None);
    }

    #[test]
    fn unrelated_text_containing_limit_does_not_match() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let text = "The disk quota limit is configured to 10GB per user in settings.";
        assert!(
            parse("codex", text, now).is_none(),
            "the word 'limit' alone must not trigger a quota/rate-limit classification"
        );
    }

    #[test]
    fn malformed_reset_text_preserves_classification_but_leaves_reset_unset() {
        let now = utc(2026, Month::January, 1, 0, 0);
        // Same shape as a real Codex reset string, but with an unrecognized
        // month name -- classification (from the independent "usage limit"
        // phrase) must survive; the reset time must not be guessed.
        let text = "You've hit your usage limit... try again at Blah 23rd, 2026 9:01 PM.";
        let parsed = parse("codex", text, now).unwrap();
        assert_eq!(parsed.kind, FailureKind::QuotaExhausted);
        assert_eq!(parsed.reset_at, None);
        assert_eq!(parsed.unresolved_timezone, None);
    }

    #[test]
    fn evidence_is_preserved_and_nonempty_for_every_match() {
        let now = utc(2026, Month::January, 1, 0, 0);
        for fixture in [
            CODEX_FULL_RESET,
            CODEX_ADMIN_VARIANT,
            CODEX_TIME_ONLY,
            CLAUDE_TZ_RESET,
            CLAUDE_WEEKLY_LIMIT,
            CLAUDE_GENERIC_RATE_LIMIT,
            CLAUDE_TRANSIENT_THROTTLE,
        ] {
            let parsed = parse("test", fixture, now).unwrap();
            assert!(!parsed.matched_evidence.trim().is_empty());
        }
    }

    // ── AGY: quota/auth classification (TICKET-107) ─────────────────────
    //
    // Real evidence exists on this host only for the auth-failure text
    // (see PROVENANCE.md, `agy_auth_not_logged_in.txt`). No real AGY
    // quota-exhaustion capture exists yet, so the quota-branch tests below
    // exercise the literal strings already shipped in `agy_quota_re`
    // rather than an external fixture -- see PROVENANCE.md for the caveat.

    #[test]
    fn agy_resource_exhausted_classifies_as_quota_exhaustion() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("agy-main", "Error: RESOURCE_EXHAUSTED", now).unwrap();
        assert_eq!(parsed.kind, FailureKind::QuotaExhausted);
        assert!(!parsed.retryable);
        assert_eq!(parsed.confidence, Confidence::High);
    }

    #[test]
    fn agy_contextual_code_429_classifies_as_quota_exhaustion() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("agy-second", "request failed with code 429", now).unwrap();
        assert_eq!(parsed.kind, FailureKind::QuotaExhausted);
    }

    #[test]
    fn agy_individual_quota_reached_classifies_as_quota_exhaustion() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("agy", "Individual quota reached. Resets in 2h 15m.", now).unwrap();
        assert_eq!(parsed.kind, FailureKind::QuotaExhausted);
        assert_eq!(parsed.reset_at.as_deref(), Some("2026-01-01T02:15:00Z"));
    }

    #[test]
    fn agy_auth_failure_classifies_as_authentication_error() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse("agy-main", AGY_AUTH_NOT_LOGGED_IN, now).unwrap();
        assert_eq!(parsed.kind, FailureKind::AuthenticationError);
        assert!(parsed.retryable);
        assert_eq!(parsed.confidence, Confidence::High);
    }

    #[test]
    fn agy_naked_429_without_context_does_not_match() {
        let now = utc(2026, Month::January, 1, 0, 0);
        // The regex requires the literal phrase "code 429" -- a bare 429
        // digit sequence elsewhere in unrelated text must not classify.
        let text = "The bus route 429 was delayed this morning.";
        assert!(
            parse("agy-main", text, now).is_none(),
            "a naked 429 unrelated to an HTTP/error code must not classify as quota exhaustion"
        );
    }

    #[test]
    fn agy_unknown_empty_failure_returns_none() {
        let now = utc(2026, Month::January, 1, 0, 0);
        assert!(parse("agy-main", "", now).is_none());
        assert!(parse("agy-second", "process exited with no output", now).is_none());
    }

    #[test]
    fn context_keyword_without_limit_verbiage_is_not_matched() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let text = "The context menu lists available options for this run.";
        assert!(parse("codex", text, now).is_none());
    }

    #[test]
    fn too_verbose_next_to_token_limit_is_not_context_limit() {
        // Issue #437: a bare `too` (as in "too verbose") must not combine with
        // an unrelated "token limit" mention across the 80-char window to
        // fabricate a context-window exhaustion. Such a false positive would
        // otherwise disable the model/pool (see availability.rs).
        let now = utc(2026, Month::January, 1, 0, 0);
        let text = "The output is too verbose; token limit warnings may follow";
        assert!(
            parse("codex", text, now).is_none(),
            "a generic 'too verbose' note next to 'token limit' must not be classified as context exhaustion"
        );
    }

    #[test]
    fn prompt_too_long_is_classified_as_context_limit() {
        // The explicit `too long` phrase (not the bare `too`) must still match.
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse(
            "claude",
            "The prompt is too long for the model context window",
            now,
        )
        .unwrap();
        assert_eq!(parsed.kind, FailureKind::ContextLimitExceeded);
    }

    #[test]
    fn token_too_many_is_classified_as_context_limit() {
        let now = utc(2026, Month::January, 1, 0, 0);
        let parsed = parse(
            "codex",
            "request failed: too many tokens for the input context length",
            now,
        )
        .unwrap();
        assert_eq!(parsed.kind, FailureKind::ContextLimitExceeded);
    }
}
