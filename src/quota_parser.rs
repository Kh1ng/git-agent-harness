//! TICKET-066: parse rate-limit and quota-reset signals from backend
//! failure output.
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
    /// for quota exhaustion (nothing short-term fixes it); `true` for
    /// rate-limiting (it resolves itself).
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

fn transient_not_usage_limit_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)not your usage limit").unwrap())
}

fn usage_or_weekly_limit_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(usage limit|weekly limit)").unwrap())
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
}
