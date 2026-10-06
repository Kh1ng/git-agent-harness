use anyhow::{Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

const STATUS_MARKER: &str = "__GAH_HTTP_STATUS__:";

pub struct CurlResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

fn config_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

/// Sends one coordinator HTTP request without placing credentials or JSON in argv.
pub fn request(
    method: &str,
    url: &str,
    body: Option<&str>,
    token: Option<&str>,
    timeout_secs: u32,
) -> Result<CurlResponse> {
    request_with_idempotency_key(method, url, body, token, timeout_secs, None)
}

pub fn request_with_idempotency_key(
    method: &str,
    url: &str,
    body: Option<&str>,
    token: Option<&str>,
    timeout_secs: u32,
    idempotency_key: Option<&str>,
) -> Result<CurlResponse> {
    if idempotency_key.is_some_and(|key| {
        !(16..=128).contains(&key.len())
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    }) {
        anyhow::bail!("invalid idempotency key")
    }
    let mut cmd = Command::new("curl");
    cmd.args([
        "-q",
        "-sS",
        "--max-time",
        &timeout_secs.to_string(),
        "-K",
        "-",
        "-w",
        &format!("\n{STATUS_MARKER}%{{http_code}}\n"),
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    crate::runner::process::arm_child_pdeathsig(&mut cmd);
    let mut child = cmd
        .spawn()
        .context("spawning curl for coordinator request")?;
    if let Some(mut stdin) = child.stdin.take() {
        let mut config = format!(
            "silent\nurl = \"{}\"\nrequest = \"{}\"\n",
            config_value(url),
            config_value(method)
        );
        if let Some(body) = body {
            config.push_str("header = \"Content-Type: application/json\"\n");
            config.push_str(&format!("data = \"{}\"\n", config_value(body)));
        }
        if let Some(ca) = std::env::var("GAH_COORDINATOR_CA_CERT")
            .ok()
            .filter(|value| !value.is_empty())
        {
            config.push_str(&format!("cacert = \"{}\"\n", config_value(&ca)));
        } else if std::env::var("GAH_COORDINATOR_INSECURE_TLS").as_deref() == Ok("1") {
            config.push_str("insecure\n");
        }
        if let Some(token) = token {
            config.push_str(&format!(
                "header = \"Authorization: Bearer {}\"\n",
                config_value(token)
            ));
        }
        if let Some(key) = idempotency_key {
            config.push_str(&format!(
                "header = \"Idempotency-Key: {}\"\n",
                config_value(key)
            ));
        }
        stdin.write_all(config.as_bytes())?;
    }
    let output = child.wait_with_output().context("waiting for curl")?;
    if !output.status.success() {
        anyhow::bail!(
            "coordinator request failed (curl exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let marker = stdout
        .rfind(STATUS_MARKER)
        .ok_or_else(|| anyhow::anyhow!("curl output missing status marker"))?;
    Ok(CurlResponse {
        status: stdout[marker + STATUS_MARKER.len()..]
            .trim()
            .parse()
            .context("parsing HTTP status from curl output")?,
        body: stdout[..marker].trim_end().as_bytes().to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curl_config_values_cannot_inject_directives() {
        assert_eq!(config_value("a\nheader = \"x\""), "a\\nheader = \\\"x\\\"");
    }

    #[test]
    fn idempotency_keys_are_restricted_before_reaching_curl() {
        assert!(request_with_idempotency_key(
            "POST",
            "http://unused",
            None,
            None,
            1,
            Some("short")
        )
        .is_err());
        assert!(request_with_idempotency_key(
            "POST",
            "http://unused",
            None,
            None,
            1,
            Some("invalid-key-000\nheader")
        )
        .is_err());
    }
}
