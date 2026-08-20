//! Advisory claim-safety preflight for `gah loop` startup (issue #881).
//!
//! Queries the central node's `GET /api/registry/fleet?profile=<name>` for
//! another node already reporting active claims/work on this profile
//! before starting. This is advisory only: the registry's node-staleness
//! window (`NODE_STALE_AFTER_MS`, 30 minutes) and the inherent TOCTOU race
//! between this check and actually starting work mean it cannot guarantee
//! safety against two nodes running the same profile -- that requires real
//! claim arbitration (tracked separately, issue #882). What this buys for
//! near-zero cost: turning "silently corrupt two worktrees" into "loud
//! refusal in the common case."
//!
//! Unconfigured (`registry_central_url` unset) skips the check entirely --
//! no behavior change for single-node setups. A failed check (network,
//! parse error) never blocks startup either: this is advisory, and failing
//! closed on a flaky registry would be worse than the problem it exists to
//! catch.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::process::{Command, Stdio};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreflightMode {
    /// Print a warning and proceed. Default when a central URL is
    /// configured but the mode isn't -- safer during initial fleet rollout.
    #[default]
    Warn,
    /// Abort startup with a non-zero exit.
    Refuse,
}

impl PreflightMode {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.to_ascii_lowercase().as_str() {
            "warn" => Ok(Self::Warn),
            "refuse" => Ok(Self::Refuse),
            other => anyhow::bail!(
                "invalid registry_preflight_mode '{other}': expected 'warn' or 'refuse'"
            ),
        }
    }
}

#[derive(Debug, Deserialize)]
struct NodeObservation {
    node_id: String,
    display_name: String,
    #[serde(default)]
    active_claims: Vec<serde_json::Value>,
    #[serde(default)]
    active_work: Vec<serde_json::Value>,
}

pub struct Collision {
    pub node_id: String,
    pub display_name: String,
    pub active_claim_count: usize,
    pub active_work_count: usize,
}

/// Runner abstraction so tests can substitute the curl subprocess with a
/// canned response instead of standing up a real HTTP server.
pub trait FleetQuerier {
    fn query(&self, url: &str, token: Option<&str>, timeout_secs: u32) -> Result<Vec<u8>>;
}

pub struct CurlFleetQuerier;

impl FleetQuerier for CurlFleetQuerier {
    fn query(&self, url: &str, token: Option<&str>, timeout_secs: u32) -> Result<Vec<u8>> {
        // Config-from-stdin (`-K -`) keeps the Bearer token out of argv/ps,
        // matching the existing curl-subprocess pattern in
        // src/usage/vibe_admin.rs. pdeathsig is defense-in-depth so the
        // kernel reaps this if the parent dies before curl returns.
        let mut cmd = Command::new("curl");
        cmd.args(["-sS", "--max-time", &timeout_secs.to_string(), "-K", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        crate::runner::process::arm_child_pdeathsig(&mut cmd);
        let mut child = cmd.spawn().context("spawning curl for fleet preflight")?;
        if let Some(mut stdin) = child.stdin.take() {
            let escaped_url = url.replace('\\', "\\\\").replace('"', "\\\"");
            let mut config = format!("silent\nfail\nurl = \"{escaped_url}\"\n");
            if let Some(ca) = std::env::var("GAH_COORDINATOR_CA_CERT").ok().filter(|s| !s.is_empty()) {
                let escaped_ca = ca.replace('\\', "\\\\").replace('"', "\\\"");
                config.push_str(&format!("cacert = \"{escaped_ca}\"\n"));
            } else if std::env::var("GAH_COORDINATOR_INSECURE_TLS").ok().as_deref() == Some("1") {
                config.push_str("insecure\n");
            }
            if let Some(t) = token {
                let escaped = t.replace('\\', "\\\\").replace('"', "\\\"");
                config.push_str(&format!("header = \"Authorization: Bearer {escaped}\"\n"));
            }
            stdin.write_all(config.as_bytes())?;
        }
        let output = child.wait_with_output().context("waiting for curl")?;
        if !output.status.success() {
            anyhow::bail!(
                "fleet preflight request failed (curl exit {:?}): {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(output.stdout)
    }
}

fn query_collisions(
    querier: &dyn FleetQuerier,
    central_url: &str,
    profile: &str,
    token: Option<&str>,
    timeout_secs: u32,
) -> Result<Vec<Collision>> {
    let mut url = Url::parse(central_url).context("parsing registry_central_url")?;
    url.set_path(&format!(
        "{}/api/registry/fleet",
        url.path().trim_end_matches('/')
    ));
    url.query_pairs_mut().append_pair("profile", profile);

    let body = querier.query(url.as_str(), token, timeout_secs)?;
    let nodes: Vec<NodeObservation> =
        serde_json::from_slice(&body).context("parsing fleet response")?;
    Ok(nodes
        .into_iter()
        .filter(|n| !n.active_claims.is_empty() || !n.active_work.is_empty())
        .map(|n| Collision {
            node_id: n.node_id,
            display_name: n.display_name,
            active_claim_count: n.active_claims.len(),
            active_work_count: n.active_work.len(),
        })
        .collect())
}

/// `Ok(())` means proceed (no collision, check skipped, or check itself
/// failed). `Err` means startup should abort -- only possible with
/// `PreflightMode::Refuse` and at least one other node reporting active
/// work.
pub fn check(
    querier: &dyn FleetQuerier,
    central_url: &str,
    profile: &str,
    token: Option<&str>,
    mode: PreflightMode,
) -> Result<()> {
    let collisions = match query_collisions(querier, central_url, profile, token, 10) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "gah loop: fleet preflight check failed ({e}), proceeding anyway (advisory only)"
            );
            return Ok(());
        }
    };
    if collisions.is_empty() {
        return Ok(());
    }
    let summary = collisions
        .iter()
        .map(|c| {
            format!(
                "{} ({}): {} claim(s), {} work item(s)",
                c.display_name, c.node_id, c.active_claim_count, c.active_work_count
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    match mode {
        PreflightMode::Warn => {
            eprintln!(
                "gah loop: WARNING -- other node(s) report active work on profile '{profile}': {summary}. \
                 Proceeding anyway (registry_preflight_mode=warn); this is advisory, not a guarantee."
            );
            Ok(())
        }
        PreflightMode::Refuse => {
            anyhow::bail!(
                "refusing to start: other node(s) report active work on profile '{profile}': {summary}. \
                 This is advisory (staleness window + TOCTOU race), not a guarantee -- override with \
                 registry_preflight_mode=warn if you're sure, or confirm via \
                 GET {central_url}/api/registry/fleet?profile={profile}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeQuerier {
        response: RefCell<std::result::Result<Vec<u8>, String>>,
        last_url: RefCell<Option<String>>,
        last_token: RefCell<Option<Option<String>>>,
    }

    impl FakeQuerier {
        fn ok(body: &str) -> Self {
            Self {
                response: RefCell::new(Ok(body.as_bytes().to_vec())),
                last_url: RefCell::new(None),
                last_token: RefCell::new(None),
            }
        }
        fn err(msg: &str) -> Self {
            Self {
                response: RefCell::new(Err(msg.to_string())),
                last_url: RefCell::new(None),
                last_token: RefCell::new(None),
            }
        }
    }

    impl FleetQuerier for FakeQuerier {
        fn query(&self, url: &str, token: Option<&str>, _timeout_secs: u32) -> Result<Vec<u8>> {
            *self.last_url.borrow_mut() = Some(url.to_string());
            *self.last_token.borrow_mut() = Some(token.map(|t| t.to_string()));
            match &*self.response.borrow() {
                Ok(body) => Ok(body.clone()),
                Err(e) => anyhow::bail!("{e}"),
            }
        }
    }

    #[test]
    fn empty_fleet_proceeds_under_either_mode() {
        let q = FakeQuerier::ok("[]");
        assert!(check(&q, "http://central:3773", "gah", None, PreflightMode::Warn).is_ok());
        assert!(check(
            &q,
            "http://central:3773",
            "gah",
            None,
            PreflightMode::Refuse
        )
        .is_ok());
    }

    #[test]
    fn collision_warns_but_proceeds_in_warn_mode() {
        let q = FakeQuerier::ok(
            r#"[{"node_id":"other","display_name":"Other Node","active_claims":[{}],"active_work":[]}]"#,
        );
        assert!(check(&q, "http://central:3773", "gah", None, PreflightMode::Warn).is_ok());
    }

    #[test]
    fn collision_refuses_in_refuse_mode() {
        let q = FakeQuerier::ok(
            r#"[{"node_id":"other","display_name":"Other Node","active_claims":[],"active_work":[{"ticket":"ticket-1"}]}]"#,
        );
        let err = check(
            &q,
            "http://central:3773",
            "gah",
            None,
            PreflightMode::Refuse,
        )
        .expect_err("must refuse when another node reports active work");
        assert!(err.to_string().contains("Other Node"));
    }

    #[test]
    fn node_with_no_active_work_is_not_a_collision() {
        let q = FakeQuerier::ok(
            r#"[{"node_id":"idle","display_name":"Idle Node","active_claims":[],"active_work":[]}]"#,
        );
        assert!(check(
            &q,
            "http://central:3773",
            "gah",
            None,
            PreflightMode::Refuse
        )
        .is_ok());
    }

    #[test]
    fn query_failure_never_blocks_startup() {
        let q = FakeQuerier::err("connection refused");
        assert!(check(
            &q,
            "http://central:3773",
            "gah",
            None,
            PreflightMode::Refuse
        )
        .is_ok());
    }

    #[test]
    fn malformed_response_never_blocks_startup() {
        let q = FakeQuerier::ok("not json");
        assert!(check(
            &q,
            "http://central:3773",
            "gah",
            None,
            PreflightMode::Refuse
        )
        .is_ok());
    }

    #[test]
    fn builds_the_expected_query_url_and_forwards_the_token() {
        let q = FakeQuerier::ok("[]");
        check(
            &q,
            "http://central:3773/",
            "my profile",
            Some("tok"),
            PreflightMode::Warn,
        )
        .unwrap();
        assert_eq!(
            q.last_url.borrow().as_deref(),
            Some("http://central:3773/api/registry/fleet?profile=my+profile")
        );
        assert_eq!(
            q.last_token.borrow().clone().unwrap(),
            Some("tok".to_string())
        );
    }

    #[test]
    fn mode_parse_accepts_known_values_and_rejects_others() {
        assert_eq!(PreflightMode::parse("warn").unwrap(), PreflightMode::Warn);
        assert_eq!(
            PreflightMode::parse("REFUSE").unwrap(),
            PreflightMode::Refuse
        );
        assert!(PreflightMode::parse("nonsense").is_err());
    }
}
