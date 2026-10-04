//! Headless Mistral console sign-in for a stored `mistral_login` source.
//! The password is sent only to Mistral's Ory login endpoint, through curl's
//! stdin config (never argv). The only output is the dashboard cookie header.
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

const FLOW: &str = "https://auth.mistral.ai/self-service/login/browser?return_to=https%3A%2F%2Fadmin.mistral.ai%2F";
const ACTION_PREFIX: &str = "https://auth.mistral.ai/self-service/login?";
const DASHBOARD_HOST: &str = "admin.mistral.ai";

pub(crate) fn sign_in(email: &str, password: &str) -> Result<String> {
    // tempfile creates an owner-only directory; the jar never outlives it.
    let dir = tempfile::tempdir().context("Mistral sign-in workspace unavailable")?;
    let jar = dir.path().join("cookies");
    let (_, flow) = request(&jar, FLOW, None)?;
    let (action, csrf_token) = login_form(&flow)?;
    let body = serde_json::json!({
        "method": "password",
        "identifier": email,
        "password": password,
        "csrf_token": csrf_token,
    });
    let (status, reply) = request(&jar, &action, Some(&body.to_string()))?;
    if status != 200 {
        return Err(rejection(status, &reply));
    }
    let jar = std::fs::read_to_string(&jar).context("Mistral sign-in returned no session")?;
    dashboard_cookie(&jar).context("Mistral sign-in returned no session")
}

/// The form action comes from the server, so it must stay on Mistral's Ory
/// host before a password is posted to it.
fn login_form(flow: &[u8]) -> Result<(String, String)> {
    let flow: Value = serde_json::from_slice(flow).context("unexpected Mistral sign-in page")?;
    let action = flow
        .pointer("/ui/action")
        .and_then(Value::as_str)
        .filter(|action| action.starts_with(ACTION_PREFIX))
        .context("unexpected Mistral sign-in page")?;
    let csrf = flow
        .pointer("/ui/nodes")
        .and_then(Value::as_array)
        .and_then(|nodes| {
            nodes.iter().find_map(|node| {
                let attributes = node.get("attributes")?;
                (attributes.get("name")?.as_str()? == "csrf_token")
                    .then(|| attributes.get("value")?.as_str())
                    .flatten()
            })
        })
        .context("unexpected Mistral sign-in page")?;
    Ok((action.to_owned(), csrf.to_owned()))
}

/// Ory's message IDs, never upstream text (it may echo the identifier).
fn rejection(status: u16, reply: &[u8]) -> anyhow::Error {
    let reply: Value = serde_json::from_slice(reply).unwrap_or(Value::Null);
    let text = reply.to_string();
    let ids: Vec<u64> = reply
        .pointer("/ui/messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| message.get("id")?.as_u64())
        .collect();
    if ids.contains(&4000006) {
        anyhow::anyhow!("auth_required: Mistral rejected the saved email or password")
    } else if text.contains("aal2") {
        anyhow::anyhow!(
            "auth_required: Mistral asked for a second factor; GAH stores only email and password"
        )
    } else {
        anyhow::anyhow!("Mistral sign-in failed (HTTP {status})")
    }
}

/// Cookies from a curl (Netscape) jar that the dashboard host would receive.
/// A session is only usable when Ory's `ory_session_*` cookie is present.
fn dashboard_cookie(jar: &str) -> Option<String> {
    let cookies: Vec<String> = jar
        .lines()
        .map(|line| line.strip_prefix("#HttpOnly_").unwrap_or(line))
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            let [domain, subdomains, _, _, _, name, value] = fields[..] else {
                return None;
            };
            let domain = domain.trim_start_matches('.');
            let applies = domain == DASHBOARD_HOST
                || (subdomains == "TRUE" && DASHBOARD_HOST.ends_with(&format!(".{domain}")));
            applies.then(|| format!("{name}={value}"))
        })
        .collect();
    cookies
        .iter()
        .any(|cookie| cookie.starts_with("ory_session_"))
        .then(|| cookies.join("; "))
}

fn request(jar: &Path, url: &str, json: Option<&str>) -> Result<(u16, Vec<u8>)> {
    let quote = |value: &str| value.replace('\\', "\\\\").replace('"', "\\\"");
    let jar = quote(&jar.to_string_lossy());
    let mut config = format!(
        "url = \"{}\"\ncookie = \"{jar}\"\ncookie-jar = \"{jar}\"\nheader = \"Accept: application/json\"\n",
        quote(url)
    );
    if let Some(json) = json {
        config.push_str(&format!(
            "header = \"Content-Type: application/json\"\ndata-raw = \"{}\"\n",
            quote(json)
        ));
    }
    let mut command = Command::new("curl");
    command.args([
        "--disable",
        "--silent",
        "--max-time",
        "15",
        "--max-filesize",
        "1048576",
        "--write-out",
        "\n%{http_code}",
        "-K",
        "-",
    ]);
    crate::runner::process::arm_child_pdeathsig(&mut command);
    let output = crate::runner::process::run_bounded_with_input(
        command,
        std::time::Duration::from_secs(20),
        config.as_bytes(),
    )
    .context("Mistral sign-in request failed or timed out")?;
    if !output.status.success() {
        bail!("Mistral sign-in request failed or timed out");
    }
    let split = output
        .stdout
        .iter()
        .rposition(|byte| *byte == b'\n')
        .context("invalid Mistral sign-in response")?;
    let status = std::str::from_utf8(&output.stdout[split + 1..])
        .ok()
        .and_then(|code| code.parse().ok())
        .context("invalid Mistral sign-in response")?;
    Ok((status, output.stdout[..split].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_is_only_posted_to_mistrals_own_login_action() {
        let flow = |action: &str| {
            serde_json::to_vec(&serde_json::json!({"ui": {"action": action, "nodes": [
                {"attributes": {"name": "identifier", "type": "text"}},
                {"attributes": {"name": "csrf_token", "type": "hidden", "value": "csrf-1"}}
            ]}}))
            .unwrap()
        };
        let (action, csrf) =
            login_form(&flow("https://auth.mistral.ai/self-service/login?flow=abc")).unwrap();
        assert_eq!(
            action,
            "https://auth.mistral.ai/self-service/login?flow=abc"
        );
        assert_eq!(csrf, "csrf-1");
        for hostile in [
            "https://evil.example/self-service/login?flow=abc",
            "https://auth.mistral.ai.evil.example/self-service/login?flow=abc",
            "http://auth.mistral.ai/self-service/login?flow=abc",
        ] {
            assert!(login_form(&flow(hostile)).is_err(), "{hostile}");
        }
    }

    #[test]
    fn rejections_use_message_ids_and_never_echo_upstream_text() {
        let reply = |body: Value| serde_json::to_vec(&body).unwrap();
        let wrong = rejection(
            400,
            &reply(serde_json::json!({"ui": {"messages": [
                {"id": 4000006, "text": "invalid credentials for someone@example.com"}
            ]}})),
        );
        assert_eq!(
            wrong.to_string(),
            "auth_required: Mistral rejected the saved email or password"
        );
        let second = rejection(
            422,
            &reply(
                serde_json::json!({"redirect_browser_to": "https://auth.mistral.ai/login?aal=aal2"}),
            ),
        );
        assert!(second.to_string().contains("second factor"));
        let other = rejection(500, b"<html>someone@example.com</html>");
        assert_eq!(other.to_string(), "Mistral sign-in failed (HTTP 500)");
    }

    #[test]
    fn dashboard_cookie_requires_an_ory_session_and_respects_cookie_scope() {
        let jar = "# Netscape HTTP Cookie File\n\
#HttpOnly_.mistral.ai\tTRUE\t/\tTRUE\t0\tory_session_abc\tsession-value\n\
auth.mistral.ai\tFALSE\t/\tTRUE\t0\tcsrf_token_x\tauth-only\n\
admin.mistral.ai\tFALSE\t/\tTRUE\t0\tcsrftoken\tadmin-value\n\
.evil.example\tTRUE\t/\tTRUE\t0\tory_session_evil\tother\n";
        assert_eq!(
            dashboard_cookie(jar).as_deref(),
            Some("ory_session_abc=session-value; csrftoken=admin-value")
        );
        assert_eq!(
            dashboard_cookie("auth.mistral.ai\tFALSE\t/\tTRUE\t0\tcsrf_token_x\tv\n"),
            None
        );
    }
}
