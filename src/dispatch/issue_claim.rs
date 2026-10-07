//! Issue claims that every loop working a repository can see.
//!
//! With `issue_claim.mode = "github_assignee"` a loop claims an issue by
//! assigning its own GitHub login and posting a claim comment. A login holds
//! an issue only while both are present: the assignee proves the login may
//! work in the repository, and the comment gives the claim a time and an
//! order. Author and time are read from the comment's own metadata, never
//! from its text, so a comment cannot claim on behalf of someone else.
//!
//! The rules, all decided by [`standing`]:
//!
//! - A claim is live for `ttl_minutes` after its comment was posted or last
//!   renewed. A dispatch that won a claim renews it while it runs (see
//!   [`IssueLease`]), so only an abandoned claim expires. A dispatch whose
//!   lease is lost stops before it publishes.
//! - An assignee with no claim comment was assigned by hand. That holds the
//!   issue until the assignee is removed.
//! - When two loops hold live claims, the first login in `priority_logins`
//!   wins, otherwise the earliest claim comment.
//! - An expired claim keeps holding the issue while an open pull request
//!   references it. Otherwise another loop may remove the stale assignee and
//!   claim the issue itself.
//!
//! The machine-local claim store still guards against two workers on one
//! machine; this module only arbitrates between logins.
//!
//! A dispatch claims its issue only once it holds a backend and node slot
//! (see [`admit_attempt`]), so a dispatch that is refused capacity never
//! announces work it will not do.

use super::attempts::{reserve_backend_attempt, BackendAdmissionGuard};
use super::DispatchArgs;
use crate::config::{IssueClaimMode, IssueClaimPolicy, Profile};
use crate::execution_identity::ExecutionIdentity;
use crate::ledger::LedgerEntry;
use crate::provider::provider_command;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// First characters of every claim comment. The rest of the first line is
/// for people reading the raw comment; no decision reads it.
const CLAIM_MARKER: &str = "<!-- gah-claim ";

/// Result of trying to claim an issue for this loop.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ClaimOutcome {
    /// This loop holds the issue and may dispatch it.
    Won,
    /// Someone else holds the issue. The text says who and why.
    Lost(String),
}

/// A dispatch ended because another login holds its issue. Not a failure:
/// the controller reports it as a skip and the ledger does not count it as
/// an attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IssueClaimLost {
    issue: String,
    reason: String,
}

impl std::fmt::Display for IssueClaimLost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "issue #{}: {}", self.issue, self.reason)
    }
}

impl std::error::Error for IssueClaimLost {}

pub(crate) fn issue_claim_lost(error: &anyhow::Error) -> Option<&IssueClaimLost> {
    error.chain().find_map(|cause| cause.downcast_ref())
}

/// Reserve the backend and node slot for one attempt and, on a dispatch's
/// first attempt, claim its issue and start renewing the claim in `lease`.
/// Later attempts first confirm the lease is still held. A refused slot fails
/// as a capacity deferral. Another login holding the issue fails with
/// [`IssueClaimLost`]; the slot is released as that error returns.
pub(super) fn admit_attempt(
    profile: &Profile,
    identity: &ExecutionIdentity,
    args: &DispatchArgs,
    attempt: u32,
    ledger: &mut LedgerEntry,
    lease: &mut Option<IssueLease>,
) -> Result<BackendAdmissionGuard> {
    if let Some(lease) = lease {
        lease.ensure_held()?;
    }
    let guard = reserve_backend_attempt(profile, identity, args.route_admission.as_ref())
        .map_err(|error| super::contextualize_capacity_deferral(error, attempt as usize))?;
    if attempt == 0 {
        *lease = claim_or_lose(profile, &args.target, ledger)?;
    }
    Ok(guard)
}

fn claim_or_lose(
    profile: &Profile,
    target: &str,
    ledger: &mut LedgerEntry,
) -> Result<Option<IssueLease>> {
    match claim_issue(profile, target)? {
        ClaimOutcome::Lost(reason) => {
            ledger.validation_result = Some(crate::ledger::gates::CLAIM_LOST.into());
            Err(IssueClaimLost {
                issue: target.to_string(),
                reason,
            }
            .into())
        }
        ClaimOutcome::Won if claims_on_provider(profile, target) => {
            Ok(Some(IssueLease::start(profile, target)))
        }
        ClaimOutcome::Won => Ok(None),
    }
}

fn claims_on_provider(profile: &Profile, target: &str) -> bool {
    profile.publishing.issue_claim.mode != IssueClaimMode::Local && is_issue_number(target)
}

/// A won claim, renewed in the background while the dispatch runs. Renewal
/// is the dispatch's proof of life: a loop that crashes stops renewing, and
/// its claim lapses `ttl_minutes` later. Dropping the lease stops renewal and
/// leaves the claim in place to lapse on its own, unless a pull request for
/// the issue is opened first.
#[derive(Debug)]
pub(super) struct IssueLease {
    issue: String,
    lost: Arc<Mutex<Option<String>>>,
    stop: Option<mpsc::Sender<()>>,
    renewer: Option<std::thread::JoinHandle<()>>,
}

impl IssueLease {
    fn start(profile: &Profile, issue: &str) -> Self {
        let lost = Arc::new(Mutex::new(None));
        let (stop, stopped) = mpsc::channel::<()>();
        let profile = profile.clone();
        let target = issue.to_string();
        let thread_lost = Arc::clone(&lost);
        let renewer = std::thread::spawn(move || {
            let policy = profile.publishing.issue_claim.clone();
            let board = GithubBoard { profile: &profile };
            while let Err(RecvTimeoutError::Timeout) =
                stopped.recv_timeout(renewal_interval(&policy))
            {
                let renewed = own_login()
                    .and_then(|me| renew(&board, &policy, &me, &target, OffsetDateTime::now_utc()));
                match renewed {
                    Ok(None) => {}
                    Ok(Some(reason)) => {
                        *thread_lost.lock().unwrap_or_else(|e| e.into_inner()) = Some(reason);
                        return;
                    }
                    // A missed renewal is retried next interval; the claim
                    // lapses only if renewals keep failing for a whole TTL.
                    Err(error) => {
                        eprintln!(
                            "warning: could not renew the claim on issue #{target}: {error:#}"
                        )
                    }
                }
            }
        });
        Self {
            issue: issue.to_string(),
            lost,
            stop: Some(stop),
            renewer: Some(renewer),
        }
    }

    /// Fails with [`IssueClaimLost`] once another loop has taken the issue
    /// or this loop's claim lapsed or was removed. The dispatch must stop:
    /// its work is no longer the work of record for the issue.
    pub(super) fn ensure_held(&self) -> Result<()> {
        match self.lost.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            Some(reason) => Err(IssueClaimLost {
                issue: self.issue.clone(),
                reason,
            }
            .into()),
            None => Ok(()),
        }
    }
}

impl Drop for IssueLease {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(renewer) = self.renewer.take() {
            let _ = renewer.join();
        }
    }
}

/// Renew three times per TTL, so two missed renewals in a row still leave
/// the claim live.
fn renewal_interval(policy: &IssueClaimPolicy) -> std::time::Duration {
    std::time::Duration::from_secs((u64::from(policy.ttl_minutes) * 60 / 3).max(1))
}

/// Extend this loop's live claim on `issue`. Returns why the claim is lost
/// when this loop no longer holds the issue, and renews nothing then.
fn renew(
    board: &impl IssueBoard,
    policy: &IssueClaimPolicy,
    me: &str,
    issue: &str,
    now: OffsetDateTime,
) -> Result<Option<String>> {
    let view = board.view(issue)?;
    match standing(&view, me, policy, now) {
        Standing::Mine => {}
        Standing::Theirs(login) => return Ok(Some(format!("lease taken over by {login}"))),
        Standing::Unclaimed | Standing::Stale(_) => {
            return Ok(Some(
                "this loop's claim lapsed or was removed while it worked".to_string(),
            ))
        }
    }
    let own = view
        .claims
        .iter()
        .filter(|claim| same_login(&claim.login, me))
        .max_by_key(|claim| claim.id)
        .context("a held claim has a claim comment")?;
    board.renew_claim(own.id, &claim_comment(me, policy, own.created_at, now))?;
    Ok(None)
}

/// Claim `target` on the provider, waiting out the verify window. Returns
/// `Won` without touching the provider when the profile keeps claims local
/// or `target` is not an issue number.
fn claim_issue(profile: &Profile, target: &str) -> Result<ClaimOutcome> {
    let policy = &profile.publishing.issue_claim;
    if policy.mode == IssueClaimMode::Local || !is_issue_number(target) {
        return Ok(ClaimOutcome::Won);
    }
    let board = GithubBoard::for_writing(profile)?;
    let me = own_login()?;
    if let Some(outcome) = stake(&board, policy, &me, target, OffsetDateTime::now_utc())? {
        return Ok(outcome);
    }
    std::thread::sleep(std::time::Duration::from_secs(policy.verify_seconds.into()));
    settle(&board, policy, &me, target, OffsetDateTime::now_utc())
}

/// Numbers of the issues another login holds, out of `issues` given as
/// `(number, assignees)`. Intake uses this to leave those issues alone.
///
/// An issue whose claim comments cannot be read counts as held. Returns an
/// empty set without touching the provider when the profile keeps claims
/// local.
pub(super) fn issues_held_by_others<'a>(
    profile: &Profile,
    issues: impl IntoIterator<Item = (&'a str, &'a [String])>,
) -> Result<HashSet<String>> {
    let policy = &profile.publishing.issue_claim;
    if policy.mode == IssueClaimMode::Local {
        return Ok(HashSet::new());
    }
    let board = GithubBoard::for_reading(profile)?;
    let me = own_login()?;
    let now = OffsetDateTime::now_utc();
    let mut held = HashSet::new();
    for (number, assignees) in issues {
        if assignees.iter().all(|login| same_login(login, &me)) {
            continue;
        }
        let view = match board.claims(number) {
            Ok(claims) => IssueClaimView {
                assignees: assignees.to_vec(),
                claims,
            },
            Err(error) => {
                eprintln!("warning: treating issue #{number} as claimed elsewhere: {error:#}");
                held.insert(number.to_string());
                continue;
            }
        };
        if matches!(standing(&view, &me, policy, now), Standing::Theirs(_)) {
            held.insert(number.to_string());
        }
    }
    Ok(held)
}

/// One claim comment, as GitHub recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ClaimComment {
    id: u64,
    login: String,
    created_at: OffsetDateTime,
    /// When the claim was last renewed: GitHub's `updated_at`, which moves
    /// only when the comment is edited. Equals `created_at` until then.
    renewed_at: OffsetDateTime,
}

/// The provider facts a claim decision reads.
#[derive(Debug, Clone, Default)]
struct IssueClaimView {
    assignees: Vec<String>,
    claims: Vec<ClaimComment>,
}

/// Who holds an issue, from one loop's point of view.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Standing {
    /// Nobody holds it.
    Unclaimed,
    /// This loop holds a live claim and wins against every other one.
    Mine,
    /// This login holds it: assigned by hand, or with the winning live claim.
    Theirs(String),
    /// Only these other logins are assigned, and their claims have expired.
    /// The issue is free once no open pull request references it.
    Stale(Vec<String>),
}

fn standing(
    view: &IssueClaimView,
    me: &str,
    policy: &IssueClaimPolicy,
    now: OffsetDateTime,
) -> Standing {
    let ttl = time::Duration::minutes(policy.ttl_minutes.into());
    let mut live: Vec<(&str, u64)> = Vec::new();
    let mut stale = Vec::new();
    for login in &view.assignees {
        let latest_claim = view
            .claims
            .iter()
            .filter(|claim| same_login(&claim.login, login))
            .max_by_key(|claim| claim.id);
        let mine = same_login(login, me);
        match latest_claim {
            Some(claim) if now - claim.renewed_at < ttl => live.push((login, claim.id)),
            Some(_) if !mine => stale.push(login.clone()),
            None if !mine => return Standing::Theirs(login.clone()),
            // This loop's own expired claim or hand assignment holds nothing
            // against itself; it claims afresh.
            _ => {}
        }
    }
    let winner = policy
        .priority_logins
        .iter()
        .find_map(|preferred| live.iter().find(|(login, _)| same_login(login, preferred)))
        .or_else(|| live.iter().min_by_key(|(_, claim_id)| *claim_id));
    match winner {
        Some((login, _)) if same_login(login, me) => Standing::Mine,
        Some((login, _)) => Standing::Theirs(login.to_string()),
        None if stale.is_empty() => Standing::Unclaimed,
        None => Standing::Stale(stale),
    }
}

/// The provider operations the claim protocol performs. A seam so two loops
/// can race over one in-memory issue in tests.
trait IssueBoard {
    fn view(&self, issue: &str) -> Result<IssueClaimView>;
    fn assign(&self, issue: &str, login: &str) -> Result<()>;
    fn unassign(&self, issue: &str, logins: &[String]) -> Result<()>;
    fn post_claim(&self, issue: &str, body: &str) -> Result<()>;
    fn renew_claim(&self, comment_id: u64, body: &str) -> Result<()>;
    fn has_open_pull_request(&self, issue: &str) -> Result<bool>;
}

/// First half of a claim: decide from the issue as it stands, and if it is
/// free, assign this loop and post its claim comment. `None` means the claim
/// was posted and must be confirmed by [`settle`] after the verify window.
fn stake(
    board: &impl IssueBoard,
    policy: &IssueClaimPolicy,
    me: &str,
    issue: &str,
    now: OffsetDateTime,
) -> Result<Option<ClaimOutcome>> {
    match standing(&board.view(issue)?, me, policy, now) {
        Standing::Mine => return Ok(Some(ClaimOutcome::Won)),
        Standing::Theirs(login) => {
            return Ok(Some(ClaimOutcome::Lost(format!("held by {login}"))));
        }
        Standing::Stale(logins) => {
            if board.has_open_pull_request(issue)? {
                return Ok(Some(ClaimOutcome::Lost(format!(
                    "the expired claim of {} still has an open pull request",
                    logins.join(", ")
                ))));
            }
            board.unassign(issue, &logins)?;
        }
        Standing::Unclaimed => {}
    }
    board.assign(issue, me)?;
    if let Err(error) = board.post_claim(issue, &claim_comment(me, policy, now, now)) {
        // An assignee without a claim comment reads as a hand assignment and
        // would hold the issue against every loop indefinitely.
        if let Err(cleanup) = board.unassign(issue, &[me.to_string()]) {
            eprintln!("warning: could not remove {me} from issue #{issue}: {cleanup:#}");
        }
        return Err(error);
    }
    Ok(None)
}

/// Second half of a claim: re-read the issue after the verify window. A loop
/// that did not win removes its own assignee and posts nothing further.
fn settle(
    board: &impl IssueBoard,
    policy: &IssueClaimPolicy,
    me: &str,
    issue: &str,
    now: OffsetDateTime,
) -> Result<ClaimOutcome> {
    let reason = match standing(&board.view(issue)?, me, policy, now) {
        Standing::Mine => return Ok(ClaimOutcome::Won),
        Standing::Theirs(login) => format!("yielded to {login}"),
        Standing::Unclaimed | Standing::Stale(_) => {
            "this loop's claim was removed during the verify window".to_string()
        }
    };
    board.unassign(issue, &[me.to_string()])?;
    Ok(ClaimOutcome::Lost(reason))
}

fn claim_comment(
    me: &str,
    policy: &IssueClaimPolicy,
    claimed_at: OffsetDateTime,
    now: OffsetDateTime,
) -> String {
    let expires = now + time::Duration::minutes(policy.ttl_minutes.into());
    let stamp = |at: OffsetDateTime| {
        let at = at.replace_nanosecond(0).unwrap_or(at);
        at.format(&Rfc3339).unwrap_or_else(|_| at.to_string())
    };
    let node = hostname::get()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown".to_string());
    // The timestamps also make each claim's text distinct, which
    // `post_issue_comment` needs: it never repeats an identical comment.
    format!(
        "{CLAIM_MARKER}login={me} node={node} claimed_at={claimed} expires_at={expires} -->\n\
         Claimed by the GAH loop of {me} on `{node}` until {expires}; the loop renews this \
         claim while it works. If it lapses, another loop may take this issue over, unless an \
         open pull request references it.",
        claimed = stamp(claimed_at),
        expires = stamp(expires),
    )
}

fn same_login(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn is_issue_number(target: &str) -> bool {
    !target.is_empty() && target.bytes().all(|byte| byte.is_ascii_digit())
}

/// The login `gh` is signed in as: the identity this loop claims under.
fn own_login() -> Result<String> {
    let login = gh("read own login", &["api", "user", "--jq", ".login"])?;
    let login = login.trim();
    if login.is_empty() {
        anyhow::bail!("GitHub returned no login for the signed-in account");
    }
    Ok(login.to_string())
}

fn gh(operation: &str, args: &[&str]) -> Result<String> {
    let out = provider_command("gh")
        .args(args)
        .output()
        .with_context(|| format!("launching gh to {operation}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "could not {operation}: {}",
            crate::redact::redact(&String::from_utf8_lossy(&out.stderr)).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

struct GithubBoard<'a> {
    profile: &'a Profile,
}

impl<'a> GithubBoard<'a> {
    fn for_reading(profile: &'a Profile) -> Result<Self> {
        if !profile.provider.eq_ignore_ascii_case("github") {
            anyhow::bail!(
                "issue_claim.mode = \"github_assignee\" needs a GitHub profile, not '{}'",
                profile.provider
            );
        }
        Ok(Self { profile })
    }

    fn for_writing(profile: &'a Profile) -> Result<Self> {
        if !profile.publishing.allow_issue_comments {
            anyhow::bail!(
                "issue_claim.mode = \"github_assignee\" posts claim comments, \
                 but allow_issue_comments is false"
            );
        }
        Self::for_reading(profile)
    }

    fn issue_endpoint(&self, issue: &str, suffix: &str) -> Result<String> {
        if !is_issue_number(issue) {
            anyhow::bail!("invalid issue number '{issue}': expected digits only");
        }
        Ok(format!(
            "repos/{}/issues/{issue}{suffix}",
            self.profile.repo
        ))
    }

    /// Every claim comment on the issue, whoever wrote it.
    fn claims(&self, issue: &str) -> Result<Vec<ClaimComment>> {
        let endpoint = self.issue_endpoint(issue, "/comments?per_page=100")?;
        let filter = format!(
            ".[] | select(.body | startswith(\"{CLAIM_MARKER}\")) \
             | {{id, login: (.user.login // \"\"), created_at, updated_at}}"
        );
        let lines = gh(
            "read claim comments",
            &[
                "api",
                "--method",
                "GET",
                "--paginate",
                &endpoint,
                "--jq",
                &filter,
            ],
        )?;
        lines
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let value: serde_json::Value =
                    serde_json::from_str(line).context("parsing a claim comment")?;
                let time = |field: &str| {
                    value[field]
                        .as_str()
                        .and_then(|at| OffsetDateTime::parse(at, &Rfc3339).ok())
                };
                match (
                    value["id"].as_u64(),
                    value["login"].as_str(),
                    time("created_at"),
                ) {
                    (Some(id), Some(login), Some(created_at)) => Ok(ClaimComment {
                        id,
                        login: login.to_string(),
                        created_at,
                        renewed_at: time("updated_at").map_or(created_at, |at| at.max(created_at)),
                    }),
                    _ => anyhow::bail!("claim comment is missing its id, author, or time"),
                }
            })
            .collect()
    }

    fn change_assignees(&self, issue: &str, method: &str, logins: &[String]) -> Result<String> {
        let endpoint = self.issue_endpoint(issue, "/assignees")?;
        let fields: Vec<String> = logins
            .iter()
            .map(|login| format!("assignees[]={login}"))
            .collect();
        let mut args = vec!["api", "--method", method, endpoint.as_str()];
        for field in &fields {
            args.extend(["-f", field.as_str()]);
        }
        args.extend(["--jq", ".assignees[].login"]);
        gh("change issue assignees", &args)
    }
}

impl IssueBoard for GithubBoard<'_> {
    fn view(&self, issue: &str) -> Result<IssueClaimView> {
        let endpoint = self.issue_endpoint(issue, "")?;
        let assignees = gh(
            "read issue assignees",
            &[
                "api",
                "--method",
                "GET",
                &endpoint,
                "--jq",
                ".assignees[].login",
            ],
        )?;
        Ok(IssueClaimView {
            assignees: assignees.lines().map(str::to_string).collect(),
            claims: self.claims(issue)?,
        })
    }

    fn assign(&self, issue: &str, login: &str) -> Result<()> {
        // GitHub answers success and changes nothing when the login may not
        // be assigned. Catch that here, before a claim comment is posted
        // that could never take effect.
        let assigned = self.change_assignees(issue, "POST", &[login.to_string()])?;
        if !assigned.lines().any(|line| same_login(line, login)) {
            anyhow::bail!("GitHub did not assign {login} to issue #{issue}; check its access");
        }
        Ok(())
    }

    fn unassign(&self, issue: &str, logins: &[String]) -> Result<()> {
        self.change_assignees(issue, "DELETE", logins).map(drop)
    }

    fn post_claim(&self, issue: &str, body: &str) -> Result<()> {
        crate::provider::post_issue_comment(self.profile, issue, body)
    }

    /// Editing the claim comment moves its `updated_at`, which is the
    /// renewal. An edit notifies nobody, unlike a fresh comment.
    fn renew_claim(&self, comment_id: u64, body: &str) -> Result<()> {
        let endpoint = format!("repos/{}/issues/comments/{comment_id}", self.profile.repo);
        let body = format!("body={body}");
        gh(
            "renew claim comment",
            &[
                "api", "--method", "PATCH", &endpoint, "-f", &body, "--silent",
            ],
        )
        .map(drop)
    }

    fn has_open_pull_request(&self, issue: &str) -> Result<bool> {
        let aliases = crate::ledger::work_id_aliases(&format!("#{issue}"));
        Ok(crate::sync::fetch_active_mrs(self.profile)?
            .iter()
            .any(|mr| {
                mr.work_id
                    .as_deref()
                    .is_some_and(|id| aliases.iter().any(|alias| alias == id))
                    && !matches!(
                        crate::sync::classify(mr),
                        "MERGED" | "CLOSED_UNMERGED" | "STALE"
                    )
            }))
    }
}

#[cfg(test)]
#[path = "issue_claim/tests.rs"]
mod tests;
