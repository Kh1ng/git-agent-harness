use super::{validate_plan, PmPlanArtifact};
use crate::config::{GahConfig, Profile};
use crate::ledger::{self, FailureClass, FailureStage, LedgerEntry};
use crate::models::PlannerWorkPacket;
use crate::provider::{self, ProviderIssue};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

const PUBLICATION_SCHEMA_VERSION: u32 = 1;
const MAX_ARTIFACT_BYTES: usize = 400_000;
const MAX_PROVIDER_TITLE_CHARS: usize = 255;
const MAX_PROVIDER_BODY_BYTES: usize = 60_000;

trait IssuePublisher {
    fn get_issue(&mut self, number: &str) -> Result<ProviderIssue>;
    fn list_issues(&mut self) -> Result<Vec<ProviderIssue>>;
    fn list_labels(&mut self) -> Result<Vec<String>>;
    fn create_issue(&mut self, title: &str, body: &str, labels: &[String])
        -> Result<ProviderIssue>;
    fn link_child(&mut self, parent_number: &str, child: &ProviderIssue) -> Result<()>;
    fn link_dependency(&mut self, dependency: &ProviderIssue, child: &ProviderIssue) -> Result<()>;
}

struct CliIssuePublisher<'a> {
    profile: &'a Profile,
}

impl IssuePublisher for CliIssuePublisher<'_> {
    fn get_issue(&mut self, number: &str) -> Result<ProviderIssue> {
        provider::get_provider_issue(self.profile, number)
    }

    fn list_issues(&mut self) -> Result<Vec<ProviderIssue>> {
        provider::list_provider_issues(self.profile)
    }

    fn list_labels(&mut self) -> Result<Vec<String>> {
        provider::list_provider_label_names(self.profile)
    }

    fn create_issue(
        &mut self,
        title: &str,
        body: &str,
        labels: &[String],
    ) -> Result<ProviderIssue> {
        provider::create_provider_issue(self.profile, title, body, labels)
    }

    fn link_child(&mut self, parent_number: &str, child: &ProviderIssue) -> Result<()> {
        provider::link_provider_child(self.profile, parent_number, child)
    }

    fn link_dependency(&mut self, dependency: &ProviderIssue, child: &ProviderIssue) -> Result<()> {
        provider::link_provider_dependency(self.profile, dependency, child)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PublishedChild {
    key: String,
    issue_id: String,
    issue_number: String,
    url: String,
    #[serde(default)]
    parent_linked: bool,
    #[serde(default)]
    linked_dependencies: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PublicationState {
    schema_version: u32,
    plan_fingerprint: String,
    profile: String,
    repo: String,
    source_issue_number: String,
    status: String,
    #[serde(default)]
    children: BTreeMap<String, PublishedChild>,
}

struct PublishContext<'a> {
    cfg: &'a GahConfig,
    profile_name: &'a str,
    profile: &'a Profile,
    artifact: &'a PmPlanArtifact,
    source_issue_number: &'a str,
    fingerprint: &'a str,
    order: &'a [usize],
    state_path: &'a Path,
    child_depth: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PmPublicationSummary {
    pub(crate) plan_fingerprint: String,
    pub(crate) state_path: PathBuf,
    pub(crate) source_issue_number: String,
    pub(crate) child_issue_numbers: Vec<String>,
    pub(crate) child_depth: u32,
    pub(crate) already_published: bool,
}

struct MutationLedgerContext<'a> {
    cfg: &'a GahConfig,
    profile_name: &'a str,
    profile: &'a Profile,
    source_issue_number: &'a str,
}

pub(crate) fn publish_plan(
    cfg: &GahConfig,
    profile_name: &str,
    profile: &Profile,
    plan_path: &Path,
    dry_run: bool,
) -> Result<PmPublicationSummary> {
    let bytes = fs::read(plan_path)
        .with_context(|| format!("reading PM plan artifact: {}", plan_path.display()))?;
    if bytes.len() > MAX_ARTIFACT_BYTES {
        anyhow::bail!("PM plan artifact exceeds {MAX_ARTIFACT_BYTES} bytes");
    }
    let artifact: PmPlanArtifact = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing PM plan artifact: {}", plan_path.display()))?;
    validate_publication_contract(profile, &artifact)?;
    validate_plan(&artifact.plan)?;
    anyhow::ensure!(
        artifact.plan.tickets.len() <= profile.publishing.pm_max_children(),
        "PM plan contains {} children, exceeding configured pm_max_children={}",
        artifact.plan.tickets.len(),
        profile.publishing.pm_max_children()
    );
    if !dry_run {
        crate::dispatch::mutation_policy::enforce_policy(profile, "edit-issue")?;
    }

    let source_issue_number = parse_issue_number(&artifact.target)?;
    let fingerprint = plan_fingerprint(&artifact)?;
    let order = topological_order(&artifact.plan.tickets)?;
    let state_path = publication_state_path(plan_path)?;
    let mut state = load_or_initialize_state(
        &state_path,
        profile_name,
        profile,
        &source_issue_number,
        &fingerprint,
    )?;
    let already_published = state.status == "complete";
    let mut publisher = CliIssuePublisher { profile };
    let source_issue = publisher.get_issue(&source_issue_number)?;
    let source_depth = decomposition_depth(&source_issue.body)?;
    anyhow::ensure!(
        source_depth < profile.publishing.pm_max_depth(),
        "source issue #{} is at PM decomposition depth {}, configured maximum is {}",
        source_issue_number,
        source_depth,
        profile.publishing.pm_max_depth()
    );
    let child_depth = source_depth + 1;
    let context = PublishContext {
        cfg,
        profile_name,
        profile,
        artifact: &artifact,
        source_issue_number: &source_issue_number,
        fingerprint: &fingerprint,
        order: &order,
        state_path: &state_path,
        child_depth,
    };
    publish_with_provider(&context, &mut state, &mut publisher, dry_run)?;
    let summary = PmPublicationSummary {
        plan_fingerprint: fingerprint,
        state_path,
        source_issue_number,
        child_issue_numbers: state
            .children
            .values()
            .map(|child| child.issue_number.clone())
            .collect(),
        child_depth,
        already_published,
    };
    if !dry_run {
        append_parent_publication_ledger(cfg, profile_name, profile, &summary)?;
    }
    Ok(summary)
}

pub(crate) fn validate_source_depth(profile: &Profile, target: &str) -> Result<u32> {
    let source_issue_number = parse_issue_number(target)?;
    let source = provider::get_provider_issue(profile, &source_issue_number)?;
    let depth = decomposition_depth(&source.body)?;
    anyhow::ensure!(
        depth < profile.publishing.pm_max_depth(),
        "source issue #{} is at PM decomposition depth {}, configured maximum is {}",
        source_issue_number,
        depth,
        profile.publishing.pm_max_depth()
    );
    Ok(depth)
}

fn append_parent_publication_ledger(
    cfg: &GahConfig,
    profile_name: &str,
    profile: &Profile,
    summary: &PmPublicationSummary,
) -> Result<()> {
    let work_id = format!("#{}", summary.source_issue_number);
    let already_recorded = ledger::read_entries(cfg)?.into_iter().any(|entry| {
        entry.profile == profile_name
            && entry.repo_id == profile.repo_id
            && entry.work_id.as_deref() == Some(work_id.as_str())
            && entry.pm_plan_fingerprint.as_deref() == Some(summary.plan_fingerprint.as_str())
            && entry.pm_publication_status.as_deref() == Some("published")
    });
    if already_recorded {
        return Ok(());
    }
    let mut entry = LedgerEntry::new(
        profile_name,
        profile,
        "control-plane",
        "pm_publish",
        &work_id,
        None,
        summary.state_path.parent(),
    );
    entry.work_id = Some(work_id);
    entry.source_issue_number = Some(summary.source_issue_number.clone());
    entry.validation_result = Some("passed".to_string());
    entry.provider_mutation_kind = Some("plan_publish".to_string());
    entry.provider_mutation_status = Some("succeeded".to_string());
    entry.pm_plan_fingerprint = Some(summary.plan_fingerprint.clone());
    entry.pm_publication_status = Some("published".to_string());
    entry.pm_child_issue_numbers = summary.child_issue_numbers.clone();
    entry.pm_decomposition_depth = Some(summary.child_depth);
    entry.pm_publication_state_path = Some(summary.state_path.display().to_string());
    ledger::append(cfg, &entry)?;
    Ok(())
}

fn publish_with_provider(
    context: &PublishContext<'_>,
    state: &mut PublicationState,
    publisher: &mut dyn IssuePublisher,
    dry_run: bool,
) -> Result<()> {
    let cfg = context.cfg;
    let profile_name = context.profile_name;
    let profile = context.profile;
    let artifact = context.artifact;
    let source_issue_number = context.source_issue_number;
    let fingerprint = context.fingerprint;
    let order = context.order;
    let state_path = context.state_path;
    let ledger_context = MutationLedgerContext {
        cfg,
        profile_name,
        profile,
        source_issue_number,
    };
    let source_issue = ensure_source_open(publisher, source_issue_number)?;
    let existing_labels = publisher
        .list_labels()?
        .into_iter()
        .map(|label| (label.to_ascii_lowercase(), label))
        .collect::<BTreeMap<_, _>>();
    let mut provider_issues = publisher.list_issues()?;

    if dry_run {
        println!(
            "PM publish dry run: profile={} repo={} source=#{} fingerprint={}",
            profile_name, profile.repo, source_issue_number, fingerprint
        );
    }

    for &index in order {
        let ticket = &artifact.plan.tickets[index];
        let ticket_key = ticket.key.trim();
        let marker = child_marker(fingerprint, ticket_key);
        let mut matches = provider_issues
            .iter()
            .filter(|issue| issue.body.contains(&marker))
            .cloned()
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            anyhow::bail!(
                "multiple provider issues carry the PM marker for plan key '{}'",
                ticket_key
            );
        }
        let mut configured_labels = labels_for_ticket(profile, ticket, &existing_labels)?;

        if dry_run {
            let disposition = matches
                .first()
                .map(|issue| format!("existing {}", issue.url))
                .unwrap_or_else(|| "would create".to_string());
            println!(
                "- {}: {} [{}] labels={}",
                ticket_key,
                ticket.title,
                disposition,
                configured_labels.join(",")
            );
            continue;
        }

        let (child, created_now) = if let Some(existing) = matches.pop() {
            (existing, false)
        } else if let Some(saved) = state.children.get(ticket_key) {
            let existing = publisher.get_issue(&saved.issue_number)?;
            if !existing.body.contains(&marker) {
                anyhow::bail!(
                    "saved child {} no longer carries the expected PM marker",
                    existing.url
                );
            }
            (existing, false)
        } else {
            ensure_source_open(publisher, source_issue_number)?;
            // Close the read/create race as far as the provider API permits:
            // another publisher that committed this exact fingerprint is
            // observed before this process attempts its POST.
            if let Some(existing) = publisher
                .list_issues()?
                .into_iter()
                .find(|issue| issue.body.contains(&marker))
            {
                (existing, false)
            } else {
                let current_labels = publisher
                    .list_labels()?
                    .into_iter()
                    .map(|label| (label.to_ascii_lowercase(), label))
                    .collect::<BTreeMap<_, _>>();
                configured_labels = labels_for_ticket(profile, ticket, &current_labels)?;
                let body = render_issue_body(
                    &source_issue.url,
                    fingerprint,
                    ticket,
                    state,
                    context.child_depth,
                );
                if ticket.title.chars().count() > MAX_PROVIDER_TITLE_CHARS {
                    anyhow::bail!(
                        "PM child '{}' title exceeds provider limit of {} characters",
                        ticket_key,
                        MAX_PROVIDER_TITLE_CHARS
                    );
                }
                if body.len() > MAX_PROVIDER_BODY_BYTES {
                    anyhow::bail!(
                        "PM child '{}' body exceeds provider safety limit of {} bytes",
                        ticket_key,
                        MAX_PROVIDER_BODY_BYTES
                    );
                }
                ensure_source_open(publisher, source_issue_number)?;
                append_mutation_ledger(
                    &ledger_context,
                    ticket,
                    "issue_create",
                    "attempted",
                    None,
                    None,
                )?;
                let created = match publisher.create_issue(&ticket.title, &body, &configured_labels)
                {
                    Ok(created) => created,
                    Err(error) => {
                        // A provider can commit a POST and still lose the response.
                        // Re-read the marker before declaring failure or retrying.
                        let recovered = publisher
                            .list_issues()?
                            .into_iter()
                            .find(|issue| issue.body.contains(&marker));
                        if let Some(recovered) = recovered {
                            recovered
                        } else {
                            append_mutation_ledger(
                                &ledger_context,
                                ticket,
                                "issue_create",
                                "failed",
                                None,
                                Some(&error.to_string()),
                            )?;
                            state.status = "partial".to_string();
                            write_state_atomic(state_path, state)?;
                            return Err(error).context("creating PM child issue");
                        }
                    }
                };
                (created, true)
            }
        };

        if created_now {
            append_mutation_ledger(
                &ledger_context,
                ticket,
                "issue_create",
                "succeeded",
                Some(&child.url),
                None,
            )?;
        }
        provider_issues.retain(|issue| issue.number != child.number);
        provider_issues.push(child.clone());
        state
            .children
            .entry(ticket_key.to_string())
            .or_insert_with(|| PublishedChild {
                key: ticket_key.to_string(),
                issue_id: child.id.clone(),
                issue_number: child.number.clone(),
                url: child.url.clone(),
                parent_linked: false,
                linked_dependencies: BTreeSet::new(),
            });
        state.status = "partial".to_string();
        write_state_atomic(state_path, state)?;

        if !state.children[ticket_key].parent_linked {
            ensure_source_open(publisher, source_issue_number)?;
            append_mutation_ledger(
                &ledger_context,
                ticket,
                "parent_link",
                "attempted",
                Some(&child.url),
                None,
            )?;
            if let Err(error) = publisher.link_child(source_issue_number, &child) {
                append_mutation_ledger(
                    &ledger_context,
                    ticket,
                    "parent_link",
                    "failed",
                    Some(&child.url),
                    Some(&error.to_string()),
                )?;
                state.status = "partial".to_string();
                write_state_atomic(state_path, state)?;
                return Err(error).context("linking PM child to source issue");
            }
            state
                .children
                .get_mut(ticket_key)
                .expect("child inserted above")
                .parent_linked = true;
            append_mutation_ledger(
                &ledger_context,
                ticket,
                "parent_link",
                "succeeded",
                Some(&child.url),
                None,
            )?;
            write_state_atomic(state_path, state)?;
        }

        for dependency_key in &ticket.depends_on {
            let dependency_key = dependency_key.trim();
            if state.children[ticket_key]
                .linked_dependencies
                .contains(dependency_key)
            {
                continue;
            }
            let dependency_state = state.children.get(dependency_key).ok_or_else(|| {
                anyhow::anyhow!(
                    "dependency '{}' was not published before '{}'",
                    dependency_key,
                    ticket_key
                )
            })?;
            let dependency = publisher.get_issue(&dependency_state.issue_number)?;
            ensure_source_open(publisher, source_issue_number)?;
            append_mutation_ledger(
                &ledger_context,
                ticket,
                "dependency_link",
                "attempted",
                Some(&child.url),
                None,
            )?;
            if let Err(error) = publisher.link_dependency(&dependency, &child) {
                append_mutation_ledger(
                    &ledger_context,
                    ticket,
                    "dependency_link",
                    "failed",
                    Some(&child.url),
                    Some(&error.to_string()),
                )?;
                state.status = "partial".to_string();
                write_state_atomic(state_path, state)?;
                return Err(error).context("linking PM child dependency");
            }
            state
                .children
                .get_mut(ticket_key)
                .expect("child inserted above")
                .linked_dependencies
                .insert(dependency_key.to_string());
            append_mutation_ledger(
                &ledger_context,
                ticket,
                "dependency_link",
                "succeeded",
                Some(&child.url),
                None,
            )?;
            write_state_atomic(state_path, state)?;
        }
    }

    if dry_run {
        return Ok(());
    }
    state.status = "complete".to_string();
    write_state_atomic(state_path, state)?;
    println!(
        "Published {} idempotent child issue(s); state: {}",
        state.children.len(),
        state_path.display()
    );
    Ok(())
}

fn validate_publication_contract(profile: &Profile, artifact: &PmPlanArtifact) -> Result<()> {
    if artifact.schema_version != 1 {
        anyhow::bail!(
            "unsupported PM plan schema_version {} (expected 1)",
            artifact.schema_version
        );
    }
    if artifact.profile != profile.display_name {
        anyhow::bail!(
            "PM plan profile mismatch: artifact='{}' configured='{}'",
            artifact.profile,
            profile.display_name
        );
    }
    if artifact.repo != profile.repo {
        anyhow::bail!(
            "PM plan project mismatch: artifact='{}' configured='{}'",
            artifact.repo,
            profile.repo
        );
    }
    if artifact.ticket_count != artifact.plan.tickets.len() {
        anyhow::bail!("PM plan ticket_count does not match plan.tickets");
    }
    Ok(())
}

fn parse_issue_number(target: &str) -> Result<String> {
    let candidate = target
        .trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim_start_matches('#');
    if candidate.is_empty() || !candidate.chars().all(|ch| ch.is_ascii_digit()) {
        anyhow::bail!(
            "PM publication requires a canonical numeric source issue target, got '{}'",
            crate::redact::redact(target)
        );
    }
    Ok(candidate.to_string())
}

fn ensure_source_open(publisher: &mut dyn IssuePublisher, number: &str) -> Result<ProviderIssue> {
    let source = publisher.get_issue(number)?;
    if !matches!(
        source.state.to_ascii_lowercase().as_str(),
        "open" | "opened"
    ) {
        anyhow::bail!(
            "source issue {} is {}; publication stopped before provider write",
            source.url,
            source.state
        );
    }
    Ok(source)
}

fn plan_fingerprint(artifact: &PmPlanArtifact) -> Result<String> {
    let canonical = serde_json::to_vec(artifact)?;
    Ok(format!("{:x}", Sha256::digest(canonical)))
}

fn key_fingerprint(key: &str) -> String {
    format!("{:x}", Sha256::digest(key.trim().as_bytes()))
}

fn child_marker(plan_fingerprint: &str, key: &str) -> String {
    format!(
        "<!-- gah-pm-child:v1 plan={} key={} -->",
        plan_fingerprint,
        key_fingerprint(key)
    )
}

fn decomposition_depth(body: &str) -> Result<u32> {
    const PREFIX: &str = "<!-- gah-pm-depth:v1 depth=";
    let Some(start) = body.find(PREFIX) else {
        return Ok(0);
    };
    let rest = &body[start + PREFIX.len()..];
    let value = rest
        .split_once(" -->")
        .map(|(value, _)| value)
        .ok_or_else(|| anyhow::anyhow!("malformed GAH PM decomposition depth marker"))?;
    value
        .parse::<u32>()
        .context("parsing GAH PM decomposition depth marker")
}

fn labels_for_ticket(
    profile: &Profile,
    ticket: &PlannerWorkPacket,
    existing: &BTreeMap<String, String>,
) -> Result<Vec<String>> {
    let mut requested = Vec::new();
    let policy = &profile.publishing;
    if ticket.execution_disposition == "autonomous" {
        requested.push(policy.canonical_autonomous_label.clone());
    }
    if let Some(label) = policy.pm_difficulty_labels.get(&ticket.difficulty) {
        requested.push(label.clone());
    }
    if let Some(label) = policy.pm_risk_labels.get(&ticket.risk) {
        requested.push(label.clone());
    }
    if let Some(label) = policy
        .pm_execution_labels
        .get(&ticket.execution_disposition)
    {
        requested.push(label.clone());
    }
    let mut resolved = BTreeSet::new();
    for configured in requested {
        let Some(provider_label) = existing.get(&configured.to_ascii_lowercase()) else {
            anyhow::bail!(
                "configured PM label '{}' does not exist in provider project {}; create the approved label explicitly or change the profile mapping",
                configured,
                profile.repo
            );
        };
        resolved.insert(provider_label.clone());
    }
    Ok(resolved.into_iter().collect())
}

fn render_issue_body(
    source_issue_url: &str,
    fingerprint: &str,
    ticket: &PlannerWorkPacket,
    state: &PublicationState,
    child_depth: u32,
) -> String {
    let dependencies = ticket
        .depends_on
        .iter()
        .map(|key| {
            state
                .children
                .get(key.trim())
                .map(|child| child.url.clone())
                .unwrap_or_else(|| format!("plan-key:{}", key.trim()))
        })
        .collect::<Vec<_>>();
    format!(
        "{}\n<!-- gah-pm-depth:v1 depth={} -->\n\nParent: {}\nPlan key: {}\nPlan fingerprint: {}\nSummary: {}\nObjective: {}\nTask class: {}\nDifficulty: {}\nRisk: {}\nExecution disposition: {}\nRecommended routing: capability={} min_tier={}\nDependencies: {}\nDuplicate evidence: {}\nUncovered reason: {}\n\n## Affected areas\n{}\n\n## Affected files\n{}\n\n## Acceptance criteria\n{}\n\n## Verification\n{}\n",
        child_marker(fingerprint, &ticket.key),
        child_depth,
        source_issue_url,
        ticket.key,
        fingerprint,
        ticket.summary,
        ticket.objective,
        ticket.task_class,
        ticket.difficulty,
        ticket.risk,
        ticket.execution_disposition,
        ticket.recommended_routing.capability,
        ticket.recommended_routing.min_tier,
        if dependencies.is_empty() { "none".to_string() } else { dependencies.join(", ") },
        if ticket.duplicate_evidence.is_empty() { "none".to_string() } else { ticket.duplicate_evidence.join("; ") },
        ticket.uncovered_reason,
        render_list(&ticket.affected_areas),
        render_list(&ticket.affected_files),
        render_list(&ticket.acceptance_criteria),
        render_list(&ticket.verification_commands),
    )
}

fn render_list(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("- {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn topological_order(tickets: &[PlannerWorkPacket]) -> Result<Vec<usize>> {
    let indices = tickets
        .iter()
        .enumerate()
        .map(|(index, ticket)| (ticket.key.trim(), index))
        .collect::<HashMap<_, _>>();
    let mut emitted = BTreeSet::new();
    let mut order = Vec::with_capacity(tickets.len());
    while order.len() < tickets.len() {
        let mut progressed = false;
        for (index, ticket) in tickets.iter().enumerate() {
            let key = ticket.key.trim();
            if emitted.contains(key) {
                continue;
            }
            if ticket.depends_on.iter().all(|dependency| {
                let dependency = dependency.trim();
                indices.contains_key(dependency) && emitted.contains(dependency)
            }) {
                emitted.insert(key.to_string());
                order.push(index);
                progressed = true;
            }
        }
        if !progressed {
            anyhow::bail!("PM plan dependency graph is cyclic or references an unknown key");
        }
    }
    Ok(order)
}

fn publication_state_path(plan_path: &Path) -> Result<PathBuf> {
    let file_name = plan_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow::anyhow!("PM plan path has no UTF-8 file name"))?;
    Ok(plan_path.with_file_name(format!("{file_name}.publication-v1.json")))
}

fn load_or_initialize_state(
    path: &Path,
    profile_name: &str,
    profile: &Profile,
    source_issue_number: &str,
    fingerprint: &str,
) -> Result<PublicationState> {
    if path.exists() {
        let state: PublicationState = serde_json::from_slice(&fs::read(path)?)?;
        if state.schema_version != PUBLICATION_SCHEMA_VERSION
            || state.plan_fingerprint != fingerprint
            || state.profile != profile_name
            || state.repo != profile.repo
            || state.source_issue_number != source_issue_number
        {
            anyhow::bail!("PM publication state does not match this plan/profile/source");
        }
        return Ok(state);
    }
    Ok(PublicationState {
        schema_version: PUBLICATION_SCHEMA_VERSION,
        plan_fingerprint: fingerprint.to_string(),
        profile: profile_name.to_string(),
        repo: profile.repo.clone(),
        source_issue_number: source_issue_number.to_string(),
        status: "planned".to_string(),
        children: BTreeMap::new(),
    })
}

fn write_state_atomic(path: &Path, state: &PublicationState) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("pm-publication-v1.json");
    let temp = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    fs::write(&temp, serde_json::to_vec_pretty(state)?)?;
    fs::rename(&temp, path)?;
    Ok(())
}

fn append_mutation_ledger(
    context: &MutationLedgerContext<'_>,
    ticket: &PlannerWorkPacket,
    kind: &str,
    status: &str,
    url: Option<&str>,
    error: Option<&str>,
) -> Result<()> {
    let cfg = context.cfg;
    let profile_name = context.profile_name;
    let profile = context.profile;
    let source_issue_number = context.source_issue_number;
    let mut entry = LedgerEntry::new(
        profile_name,
        profile,
        "control-plane",
        "pm_publish",
        &ticket.title,
        None,
        None,
    );
    entry.work_id = Some(format!("#{source_issue_number}:{}", ticket.key.trim()));
    entry.source_issue_number = Some(source_issue_number.to_string());
    entry.work_title = Some(ticket.title.clone());
    entry.task_class = Some(ticket.task_class.clone());
    entry.difficulty = Some(ticket.difficulty.clone());
    entry.validation_result = Some(status.to_string());
    entry.provider_mutation_kind = Some(kind.to_string());
    entry.provider_mutation_status = Some(status.to_string());
    entry.provider_mutation_url = url.map(ToOwned::to_owned);
    if let Some(error) = error {
        entry.set_failure(FailureClass::HarnessError, FailureStage::Sync);
        entry.error_summary = Some(crate::redact::redact(error));
    }
    ledger::append(cfg, &entry)?;
    Ok(())
}

#[cfg(test)]
#[path = "publish/tests.rs"]
mod tests;
