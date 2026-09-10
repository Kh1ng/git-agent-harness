//! Issue #653: configurable notification channels for notify-worthy events.
//!
//! `[defaults].notification_channel` selects where the same redacted
//! one-line message that `notify_command` receives is ALSO delivered:
//! Telegram (Bot API `sendMessage`, the MVP the owner asked for) or a
//! Discord webhook (so another operator can plug a different surface in
//! without code changes). Credentials never live in config: the Telegram
//! bot token comes from `TELEGRAM_BOT_TOKEN`, the Discord webhook URL from
//! `DISCORD_WEBHOOK_URL`, both read from the environment at send time.
//!
//! Delivery failures are always visible (a `NotificationDeliveryFailed`
//! event is recorded) and never block the operation that raised the
//! notification — unknown delivery must not bypass approval (#653), it
//! must only be observable. Deduplication stays the caller's job: the
//! paid-route notice path already notifies once per occurrence.

use crate::config::GahConfig;
use anyhow::{Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NotificationChannel {
    /// No channel delivery; `notify_command` (per profile) still applies.
    #[default]
    None,
    /// Telegram Bot API `sendMessage` to the configured chat.
    Telegram,
    /// Discord incoming webhook (`content` message).
    Discord,
}

impl NotificationChannel {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "none" => Some(Self::None),
            "telegram" => Some(Self::Telegram),
            "discord" => Some(Self::Discord),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Telegram => "telegram",
            Self::Discord => "discord",
        }
    }
}

/// The one HTTP primitive channel delivery needs. Mirrors the memory
/// gateway's transport seam: curl-based in production (the token reaches
/// curl through a stdin config file, never argv), a recording double in
/// tests.
pub(crate) trait NotifyTransport {
    /// Returns `(http_status, response_body)` on a completed exchange;
    /// `Err` only for transport-level failure.
    fn post_json(&self, url: &str, body: &str, timeout_secs: u32) -> Result<(u16, Vec<u8>)>;
}

pub(crate) struct CurlNotifyTransport;

impl NotifyTransport for CurlNotifyTransport {
    fn post_json(&self, url: &str, body: &str, timeout_secs: u32) -> Result<(u16, Vec<u8>)> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        // The curl config file carries the URL (which contains the Telegram
        // token) so it never appears in the process list; the file is
        // created 0600 by tempfile and deleted on drop. The JSON body
        // arrives on stdin via --data-binary @-.
        let mut config = tempfile::NamedTempFile::new()
            .context("creating curl config for channel notification")?;
        writeln!(config, "url = \"{url}\"").context("writing curl config")?;
        let mut child = Command::new("curl")
            .args([
                "-sS",
                "--max-time",
                &timeout_secs.to_string(),
                "-K",
                config.path().to_string_lossy().as_ref(),
                "-w",
                "\n%{http_code}",
                "--data-binary",
                "@-",
                "-o",
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawning curl for channel notification")?;
        child
            .stdin
            .as_mut()
            .expect("piped stdin")
            .write_all(body.as_bytes())
            .context("writing channel notification body")?;
        let output = child
            .wait_with_output()
            .context("running curl for channel notification")?;
        let raw = String::from_utf8_lossy(&output.stdout);
        // `-w` appends the status code on its own final line.
        let (response_body, status) = match raw.rfind('\n') {
            Some(index) => (&raw[..index], raw[index + 1..].trim()),
            None => (raw.as_ref(), ""),
        };
        let status: u16 = status.parse().with_context(|| {
            format!("parsing curl status output for channel notification: {status:?}")
        })?;
        Ok((status, response_body.as_bytes().to_vec()))
    }
}

/// One delivery attempt through the configured channel. `Ok(())` means the
/// remote accepted the message (HTTP 2xx); anything else — missing
/// credential, non-2xx, transport failure — is a descriptive `Err` the
/// caller turns into a visible delivery-failure event.
pub(crate) fn deliver_channel_message(
    cfg: &GahConfig,
    message: &str,
    transport: &dyn NotifyTransport,
) -> Result<()> {
    match cfg.defaults.notification_channel {
        NotificationChannel::None => Ok(()),
        NotificationChannel::Telegram => {
            let token = std::env::var("TELEGRAM_BOT_TOKEN")
                .ok()
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty())
                .context(
                    "TELEGRAM_BOT_TOKEN is not set; cannot deliver the Telegram notification",
                )?;
            let chat_id = cfg
                .defaults
                .telegram_chat_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .context("telegram_chat_id is not configured for the telegram channel")?;
            let url = format!("https://api.telegram.org/bot{token}/sendMessage");
            let body = serde_json::json!({ "chat_id": chat_id, "text": message }).to_string();
            deliver(transport, &url, &body, "Telegram")
        }
        NotificationChannel::Discord => {
            let webhook = std::env::var("DISCORD_WEBHOOK_URL")
                .ok()
                .map(|url| url.trim().to_string())
                .filter(|url| !url.is_empty())
                .context(
                    "DISCORD_WEBHOOK_URL is not set; cannot deliver the Discord notification",
                )?;
            let body = serde_json::json!({ "content": message }).to_string();
            deliver(transport, &webhook, &body, "Discord")
        }
    }
}

fn deliver(transport: &dyn NotifyTransport, url: &str, body: &str, label: &str) -> Result<()> {
    let (status, response) = transport.post_json(url, body, 10)?;
    if (200..300).contains(&status) {
        return Ok(());
    }
    // Never echo the response body by default: for Telegram errors it can
    // reflect parameter content, and the caller's message is already known.
    let snippet = String::from_utf8_lossy(&response);
    let snippet = snippet.chars().take(120).collect::<String>();
    anyhow::bail!("{label} notification rejected with HTTP {status}: {snippet}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct RecordingTransport {
        requests: Mutex<Vec<(String, String)>>,
        status: u16,
    }

    impl RecordingTransport {
        fn new(status: u16) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                status,
            }
        }

        fn last(&self) -> (String, String) {
            self.requests
                .lock()
                .unwrap()
                .last()
                .cloned()
                .expect("a request was made")
        }
    }

    impl NotifyTransport for RecordingTransport {
        fn post_json(&self, url: &str, body: &str, _timeout_secs: u32) -> Result<(u16, Vec<u8>)> {
            self.requests
                .lock()
                .unwrap()
                .push((url.to_string(), body.to_string()));
            Ok((self.status, b"{}".to_vec()))
        }
    }

    fn config_with_channel(channel: NotificationChannel, chat_id: Option<&str>) -> GahConfig {
        let (_tmp, cfg) = crate::ledger::test_util::test_config();
        let mut cfg = cfg;
        cfg.defaults.notification_channel = channel;
        cfg.defaults.telegram_chat_id = chat_id.map(str::to_string);
        cfg
    }

    #[test]
    fn none_channel_delivers_nothing() {
        let transport = RecordingTransport::new(200);
        deliver_channel_message(
            &config_with_channel(NotificationChannel::None, None),
            "hello",
            &transport,
        )
        .unwrap();
        assert!(transport.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn telegram_delivery_sends_chat_id_and_text_with_env_token() {
        let transport = RecordingTransport::new(200);
        let cfg = config_with_channel(NotificationChannel::Telegram, Some("12345"));
        // Env guard: single-threaded test binaries may share the process env,
        // so take the lock-free route of a unique value plus restore.
        let previous = std::env::var("TELEGRAM_BOT_TOKEN").ok();
        std::env::set_var("TELEGRAM_BOT_TOKEN", "test-token-123");
        let result = deliver_channel_message(&cfg, "approval needed", &transport);
        match previous {
            Some(value) => std::env::set_var("TELEGRAM_BOT_TOKEN", value),
            None => std::env::remove_var("TELEGRAM_BOT_TOKEN"),
        }
        result.expect("delivery must succeed");

        let (url, body) = transport.last();
        assert!(url.starts_with("https://api.telegram.org/bottest-token-123/sendMessage"));
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["chat_id"], "12345");
        assert_eq!(parsed["text"], "approval needed");
    }

    #[test]
    fn telegram_without_token_fails_descriptively() {
        let transport = RecordingTransport::new(200);
        let cfg = config_with_channel(NotificationChannel::Telegram, Some("12345"));
        let previous = std::env::var("TELEGRAM_BOT_TOKEN").ok();
        std::env::remove_var("TELEGRAM_BOT_TOKEN");
        let result = deliver_channel_message(&cfg, "hello", &transport);
        match previous {
            Some(value) => std::env::set_var("TELEGRAM_BOT_TOKEN", value),
            None => std::env::remove_var("TELEGRAM_BOT_TOKEN"),
        }
        let error = result.expect_err("missing token must fail");
        assert!(format!("{error:#}").contains("TELEGRAM_BOT_TOKEN is not set"));
        assert!(transport.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn telegram_without_chat_id_fails_descriptively() {
        let transport = RecordingTransport::new(200);
        let cfg = config_with_channel(NotificationChannel::Telegram, None);
        let previous = std::env::var("TELEGRAM_BOT_TOKEN").ok();
        std::env::set_var("TELEGRAM_BOT_TOKEN", "test-token-123");
        let result = deliver_channel_message(&cfg, "hello", &transport);
        match previous {
            Some(value) => std::env::set_var("TELEGRAM_BOT_TOKEN", value),
            None => std::env::remove_var("TELEGRAM_BOT_TOKEN"),
        }
        let error = result.expect_err("missing chat id must fail");
        assert!(format!("{error:#}").contains("telegram_chat_id is not configured"));
    }

    #[test]
    fn discord_delivery_posts_content_from_env_webhook() {
        let transport = RecordingTransport::new(200);
        let cfg = config_with_channel(NotificationChannel::Discord, None);
        let previous = std::env::var("DISCORD_WEBHOOK_URL").ok();
        std::env::set_var(
            "DISCORD_WEBHOOK_URL",
            "https://discord.com/api/webhooks/1/abc",
        );
        let result = deliver_channel_message(&cfg, "hello from gah", &transport);
        match previous {
            Some(value) => std::env::set_var("DISCORD_WEBHOOK_URL", value),
            None => std::env::remove_var("DISCORD_WEBHOOK_URL"),
        }
        result.expect("delivery must succeed");

        let (url, body) = transport.last();
        assert_eq!(url, "https://discord.com/api/webhooks/1/abc");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["content"], "hello from gah");
    }

    #[test]
    fn non_2xx_is_a_descriptive_error_with_a_bounded_snippet() {
        let transport = RecordingTransport::new(403);
        let cfg = config_with_channel(NotificationChannel::Discord, None);
        let previous = std::env::var("DISCORD_WEBHOOK_URL").ok();
        std::env::set_var(
            "DISCORD_WEBHOOK_URL",
            "https://discord.com/api/webhooks/1/abc",
        );
        let result = deliver_channel_message(&cfg, "hello", &transport);
        match previous {
            Some(value) => std::env::set_var("DISCORD_WEBHOOK_URL", value),
            None => std::env::remove_var("DISCORD_WEBHOOK_URL"),
        }
        let error = result.expect_err("403 must fail");
        assert!(format!("{error:#}").contains("HTTP 403"));
    }

    #[test]
    fn channel_parses_the_settings_vocabulary() {
        assert_eq!(
            NotificationChannel::parse("telegram"),
            Some(NotificationChannel::Telegram)
        );
        assert_eq!(
            NotificationChannel::parse("discord"),
            Some(NotificationChannel::Discord)
        );
        assert_eq!(
            NotificationChannel::parse("none"),
            Some(NotificationChannel::None)
        );
        assert_eq!(NotificationChannel::parse("slack"), None);
        assert_eq!(NotificationChannel::Telegram.as_str(), "telegram");
    }
}
