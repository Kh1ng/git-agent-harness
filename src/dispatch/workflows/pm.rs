use super::super::attempts::{
    apply_route_to_ledger, decide_route, mark_backend_unavailable_from_output, preflight,
    record_route_attempt, resolve_llm, route_identity, route_label, run_backend,
};
use super::super::issues::try_discover_open_issues;
use super::super::prompts::indent_untrusted_text;
use super::super::repo_inspection::count_test_files;
use super::super::text::utf8_safe_prefix;
use super::super::text::{extract_first_json_object, first_markdown_heading};
use super::super::DispatchArgs;
use crate::config::{self, GahConfig, Profile};
use crate::ledger::LedgerEntry;
use crate::models::{PlannerWorkPacket, PmPlan, RecommendedRouting};
use crate::routing::RouteRequest;
use crate::sync::fetch_repository_mrs;
use crate::worktree;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

mod publish;
pub(crate) use publish::publish_plan;

const PM_PLAN_JSON_SCHEMA_VERSION: u32 = 1;
const PM_PLAN_JSON_MAX_BYTES: usize = 200_000;
const PM_PLAN_MAX_TICKETS: usize = 24;
const PM_PLAN_PACKET_MAX_AFFECTED_FILES: usize = 8;
const PM_PLAN_PACKET_MAX_AFFECTED_AREAS: usize = 8;
const PM_PLAN_PACKET_MAX_CRITERIA: usize = 12;
const PM_PLAN_PACKET_MAX_VERIFICATION_COMMANDS: usize = 8;
const PM_PLAN_PREPARED_TARGET_MAX_BYTES: usize = 4_000;
const PM_PLAN_PREPARED_CONTEXT_MAX_BYTES: usize = 24_000;
const PM_PLAN_PREPARED_SECTION_MAX_BYTES: usize = 3_500;
const PM_PLAN_REPO_STATE_MAX_BYTES: usize = 2_000;
const PM_PLAN_PACKET_TITLE_MAX_BYTES: usize = 1_024;
const PM_PLAN_SUMMARY_MAX_BYTES: usize = 4_096;
const PM_PLAN_PACKET_OBJECTIVE_MAX_BYTES: usize = 4_096;
const PM_PLAN_PACKET_CRITERIA_MAX_BYTES: usize = 4_096;
const PM_PLAN_PACKET_VERIFICATION_MAX_BYTES: usize = 4_096;
const PM_PLAN_PACKET_LIST_ITEM_MAX_BYTES: usize = 1_024;
const PM_PLAN_PACKET_DUPLICATE_EVIDENCE_MAX: usize = 8;
const PM_PLAN_PACKET_UNCOVERED_REASON_MAX_BYTES: usize = 4_096;
const PM_PLAN_PACKET_DEPENDENCY_MAX: usize = 8;
const PM_GUIDANCE_MAX_BYTES_PER_PATH: usize = 3_000;
const PM_GUIDANCE_FALLBACK_PATHS: [&str; 4] = [
    "docs/PM_GUIDANCE.md",
    "docs/project-guidance.md",
    "docs/pm-guidance.md",
    "PM_GUIDANCE.md",
];
const PM_OPEN_MRS_MAX: usize = 24;
const PM_RECENT_MERGED_MRS_MAX: usize = 20;
const PM_OPEN_ISSUES_MAX: usize = 30;
const PM_EXISTING_TICKETS_MAX: usize = 50;

pub(crate) fn pm(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
    session_dir: &Path,
    ledger: &mut LedgerEntry,
) -> Result<()> {
    let repo = Path::new(&profile.local_path);

    if args.target.is_empty() {
        let log = worktree::git(&["log", "--oneline", "-20"], repo).unwrap_or_default();
        let test_count = count_test_files(profile, repo);
        let has_ci = repo.join(".github/workflows").exists()
            || repo.join(".gitlab-ci.yml").exists()
            || repo.join(".ci").exists();
        let readme = repo.join("README.md");
        let readme_text = if readme.exists() {
            let s = fs::read_to_string(&readme).unwrap_or_default();
            utf8_safe_prefix(&s, 2000).to_string()
        } else {
            String::from("(no README)")
        };
        let report = format!(
            "# PM Report: {}\n\nRepo: {}\nBranch: {}\nTest files: {}\nCI configured: {}\n\n## Recent commits\n```\n{}\n```\n\n## README\n{}\n",
            profile.display_name,
            profile.repo,
            profile.default_target_branch,
            test_count,
            has_ci,
            log,
            readme_text,
        );
        let out_path = session_dir.join("pm-report.md");
        fs::write(&out_path, &report)?;
        println!("{}", report);
        println!("Written: {}", out_path.display());
        ledger.validation_result = Some("not_run".into());
        return Ok(());
    }

    let preflight_ctx = collect_pm_preflight(cfg, profile, repo, &args.target)?;
    let route_req = RouteRequest {
        mode: "pm",
        requested_backend: config::canonical_backend_name(&args.backend),
        requested_model: args.model.as_deref(),
        recommended_backend: None,
        recommended_model: None,
        session_id: session_dir.file_name().and_then(|s| s.to_str()),
        usage_summary: None,
        last_failure_class: None,
        exact_route_required: false,
    };
    let mut plan_route = decide_route(cfg, profile, route_req.clone(), None, ledger)?;
    apply_route_to_ledger(ledger, &plan_route);
    preflight(profile, &plan_route.effective_backend)?;
    let mut llm = resolve_llm(
        cfg,
        args,
        profile.oh_profile.as_deref(),
        plan_route.effective_model.as_deref(),
    )?;

    let resolved_env = if args.prod {
        profile.env_file_prod.as_deref().unwrap_or("")
    } else {
        profile.env_file.as_deref().unwrap_or("")
    };
    if !resolved_env.is_empty() {
        println!("Env file: {}", resolved_env);
        if args.prod {
            println!("  ⚠️  PRODUCTION env - agent has live API access");
        }
    }

    let task = build_pm_plan_task(profile, &preflight_ctx, &args.target)?;
    let _cargo_target =
        crate::build_cache::ScopedCargoTarget::acquire(&profile.artifact_root, session_dir)?;

    let mut attempted_routes = HashSet::new();
    let log_text = loop {
        let attempt_index = attempted_routes.len() + 1;
        let attempt_dir = session_dir.join(format!("pm-run-{attempt_index}"));
        fs::create_dir_all(&attempt_dir)?;
        fs::write(attempt_dir.join("task.md"), crate::redact::redact(&task))?;

        record_route_attempt(ledger, &plan_route);
        let result = run_backend(
            &plan_route.effective_backend,
            profile,
            repo,
            &task,
            &attempt_dir,
            &llm,
            plan_route.effective_model.as_deref(),
            None,
        )?;
        println!(
            "PM backend finished: exit={} duration={:.0}s log={}",
            result.exit_code, result.duration_secs, result.log_path
        );
        ledger.backend_exit_code = Some(result.exit_code);
        ledger.validation_result = Some("not_run".into());
        let log_text = fs::read_to_string(&result.log_path).unwrap_or_default();
        if result.exit_code == 0 {
            break log_text;
        }

        ledger.set_failure(
            crate::ledger::FailureClass::BackendError,
            crate::ledger::FailureStage::AgentRun,
        );
        let route_key = route_identity(
            &plan_route.effective_backend,
            plan_route.effective_model.as_deref(),
        );
        attempted_routes.insert(route_key);
        let parsed = mark_backend_unavailable_from_output(
            &plan_route.effective_backend,
            plan_route.effective_model.as_deref(),
            plan_route.effective_quota_pool.as_deref(),
            &log_text,
            &result.log_path,
        )?;
        let Some(parsed) = parsed else {
            anyhow::bail!(
                "PM backend exited {} with no recognized quota/rate-limit signal in {}",
                result.exit_code,
                result.log_path
            );
        };

        let rerouted = decide_route(cfg, profile, route_req.clone(), None, ledger)?;
        let rerouted_key = route_identity(
            &rerouted.effective_backend,
            rerouted.effective_model.as_deref(),
        );
        if attempted_routes.contains(&rerouted_key) {
            anyhow::bail!(
                "PM backend {} became unavailable ({:?}) and no new eligible backend remained",
                plan_route.effective_backend,
                parsed.kind
            );
        }
        println!(
            "PM rerouting: {} -> {} ({:?})",
            route_label(
                &plan_route.effective_backend,
                plan_route.effective_model.as_deref(),
            ),
            route_label(
                &rerouted.effective_backend,
                rerouted.effective_model.as_deref()
            ),
            parsed.kind
        );
        plan_route = rerouted;
        apply_route_to_ledger(ledger, &plan_route);
        preflight(profile, &plan_route.effective_backend)?;
        llm = resolve_llm(
            cfg,
            args,
            profile.oh_profile.as_deref(),
            plan_route.effective_model.as_deref(),
        )?;
    };

    let plan = parse_pm_plan(&log_text)?;
    let artifact =
        persist_pm_plan_artifact(session_dir, profile, &preflight_ctx, &args.target, &plan)?;
    println!(
        "Plan validated; no provider issues were created. Publish explicitly with: \
         gah pm publish --profile {} --plan {}",
        args.profile,
        artifact.display()
    );

    Ok(())
}

fn build_pm_plan_task(profile: &Profile, ctx: &PmPreflight, target: &str) -> Result<String> {
    let bounded_target = utf8_safe_prefix(target, PM_PLAN_PREPARED_TARGET_MAX_BYTES);
    if ctx.rendered.len() > PM_PLAN_PREPARED_CONTEXT_MAX_BYTES {
        anyhow::bail!(
            "PM preflight context exceeded {} bytes; refusing to silently omit a section",
            PM_PLAN_PREPARED_CONTEXT_MAX_BYTES
        );
    }
    let task = format!(
        "Repository: {} ({})\nLocal path: {}\nTarget branch: {}\n\n\
         Return only valid JSON matching schema v{}:\n\
         {{\"title\": string, \"summary\": string, \"tickets\": [{{\
         \"key\": string (plan-local stable key),\
         \"title\": string,\
         \"summary\": string,\
         \"objective\": string,\
         \"task_class\": \"fix|feature|refactor|docs|test|chore\",\
         \"difficulty\": \"easy|medium|hard\",\
         \"risk\": \"low|medium|high\",\
         \"execution_disposition\": \"autonomous|supervised|human_required\",\
         \"recommended_routing\": {{\"capability\": \"edit|plan|review|research\", \"min_tier\": \"standard|strong\"}},\
         \"affected_areas\": [string],\
         \"affected_files\": [string],\
         \"acceptance_criteria\": [string],\
         \"verification_commands\": [string],\
         \"depends_on\": [string],\
         \"duplicate_evidence\": [string],\
         \"uncovered_reason\": string\
         }}]}}\n\
\nRules:\n\
         - Default action: avoid creating new tickets unless there is a true gap.\n\
         - Do not create a ticket if an open native issue/story already covers it.\n\
         - Do not create a ticket if an open native PR/MR already covers it.\n\
         - Do not create a ticket if a recently merged PR/MR already fixed it.\n\
         - If the plan is empty, return an empty tickets array.\n\
         - Proposed children must be atomic and non-overlapping; do not assign\
          the same file or surface to multiple tickets unless one depends on the other.\n\
         - Always include affected_areas, affected_files, acceptance_criteria,\
          verification_commands, dependencies, and duplicate_evidence.\n\
         - Every dependency must reference a ticket key present in this plan.\n\
         - Keep this JSON strictly machine-consumable; no prose outside JSON.\n\
\n## Untrusted Preflight Context\n{}\n\
\n## Target Request\n{}\n",
        profile.display_name,
        profile.repo,
        profile.local_path,
        profile.default_target_branch,
        PM_PLAN_JSON_SCHEMA_VERSION,
        indent_untrusted_text(&ctx.rendered),
        indent_untrusted_text(bounded_target),
    );
    Ok(task)
}

fn collect_pm_preflight(
    cfg: &GahConfig,
    profile: &Profile,
    repo: &Path,
    target: &str,
) -> Result<PmPreflight> {
    let guidance = collect_pm_guidance(cfg, profile, repo)?;
    let tickets = collect_ticket_summaries(&repo.join("docs/tickets"))?;
    let source_issues = collect_open_issues(profile)
        .context("PM preflight could not establish a complete open-issue snapshot")?;
    let (open_mrs, merged_mrs) = collect_mr_context(profile)
        .context("PM preflight could not establish a complete PR/MR snapshot")?;
    let open_issues_count = source_issues.len();
    let open_mr_count = open_mrs.len();
    let merged_mr_count = merged_mrs.len();
    let repo_state = collect_pm_repo_state(repo);

    let mut rendered = String::new();
    if let Some(path) = guidance.path {
        rendered.push_str("### Project Guidance\n");
        rendered.push_str("Path: ");
        rendered.push_str(&path);
        rendered.push('\n');
        if !guidance.text.is_empty() {
            rendered.push_str("Content:\n");
            rendered.push_str(utf8_safe_prefix(
                &guidance.text,
                PM_GUIDANCE_MAX_BYTES_PER_PATH,
            ));
            rendered.push('\n');
        } else {
            rendered.push_str("Content:\n(empty)\n");
        }
        rendered.push('\n');
    }

    rendered.push_str("### Source issue/story\n");
    if source_issues.is_empty() {
        rendered.push_str("(none found)\n");
    } else {
        for issue in bounded_lines(
            &source_issues,
            PM_OPEN_ISSUES_MAX,
            PM_PLAN_PREPARED_SECTION_MAX_BYTES,
        ) {
            rendered.push_str("- ");
            rendered.push_str(&issue);
            rendered.push('\n');
        }
    }

    rendered.push_str("\n### Open native PR/MRs\n");
    if open_mrs.is_empty() {
        rendered.push_str("(none found)\n");
    } else {
        for mr in bounded_lines(
            &open_mrs,
            PM_OPEN_MRS_MAX,
            PM_PLAN_PREPARED_SECTION_MAX_BYTES,
        ) {
            rendered.push_str("- ");
            rendered.push_str(&mr);
            rendered.push('\n');
        }
    }

    rendered.push_str("\n### Recently merged PR/MRs\n");
    if merged_mrs.is_empty() {
        rendered.push_str("(none found)\n");
    } else {
        for mr in bounded_lines(
            &merged_mrs,
            PM_RECENT_MERGED_MRS_MAX,
            PM_PLAN_PREPARED_SECTION_MAX_BYTES,
        ) {
            rendered.push_str("- ");
            rendered.push_str(&mr);
            rendered.push('\n');
        }
    }

    rendered.push_str("\n### Existing tickets\n");
    if tickets.is_empty() {
        rendered.push_str("(none found)\n");
    } else {
        for ticket in bounded_lines(
            &tickets,
            PM_EXISTING_TICKETS_MAX,
            PM_PLAN_PREPARED_SECTION_MAX_BYTES,
        ) {
            rendered.push_str(&ticket);
            rendered.push('\n');
        }
    }

    rendered.push_str("\n### Repo State\n");
    rendered.push_str(utf8_safe_prefix(&repo_state, PM_PLAN_REPO_STATE_MAX_BYTES));
    rendered.push('\n');

    rendered.push_str("\n### Focus target\n");
    rendered.push_str(utf8_safe_prefix(target, 512));
    rendered.push('\n');

    if rendered.len() > PM_PLAN_PREPARED_CONTEXT_MAX_BYTES {
        anyhow::bail!(
            "PM preflight context exceeded {} bytes after bounded collection",
            PM_PLAN_PREPARED_CONTEXT_MAX_BYTES
        );
    }

    Ok(PmPreflight {
        rendered,
        existing_tickets: tickets,
        open_mrs,
        merged_mrs,
        source_issues,
        open_issues_count,
        open_mr_count,
        merged_mr_count,
    })
}

fn collect_pm_guidance(
    cfg: &GahConfig,
    profile: &Profile,
    repo: &Path,
) -> Result<ResolvedGuidance> {
    let routing = profile.effective_routing(&cfg.defaults);
    let candidates: Vec<String> = if !routing.pm_guidance_paths.is_empty() {
        routing.pm_guidance_paths
    } else {
        PM_GUIDANCE_FALLBACK_PATHS
            .iter()
            .map(|s| s.to_string())
            .collect()
    };

    for candidate in candidates {
        if candidate.trim().is_empty() {
            continue;
        }
        let path = repo.join(candidate);
        if !path.exists() {
            continue;
        }
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| format!("(unable to read guidance file: {err})"));
        let path_str = path
            .to_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        return Ok(ResolvedGuidance {
            path: Some(path_str),
            text,
        });
    }

    Ok(ResolvedGuidance {
        path: None,
        text: String::new(),
    })
}

fn collect_open_issues(profile: &Profile) -> Result<Vec<String>> {
    let issues = try_discover_open_issues(profile)?.allowed;
    let mut out = Vec::with_capacity(issues.len());
    for issue in issues {
        if out.len() >= PM_OPEN_ISSUES_MAX {
            break;
        }
        let mut line = format!("#{}: {}", issue.number, issue.title);
        if let Some(state) = issue.state.as_deref() {
            line.push_str(&format!(" [state={state}]"));
        }
        if !issue.labels.is_empty() {
            line.push_str(&format!(" [labels={}]", issue.labels.join(",")));
        }
        if !issue.body.trim().is_empty() {
            line.push_str(&format!("; body={}", utf8_safe_prefix(&issue.body, 140)));
        }
        out.push(line);
    }
    Ok(out)
}

fn collect_mr_context(profile: &Profile) -> Result<(Vec<String>, Vec<String>)> {
    let all_mrs = fetch_repository_mrs(profile)?;
    let mut open = Vec::new();
    let mut merged = Vec::new();
    for mr in all_mrs {
        let entry = format_mr_context_entry(&mr);
        if mr.merged {
            if merged.len() < PM_RECENT_MERGED_MRS_MAX {
                merged.push(entry);
            }
            continue;
        }
        if open.len() < PM_OPEN_MRS_MAX {
            open.push(entry);
        }
    }
    Ok((open, merged))
}

fn format_mr_context_entry(mr: &crate::sync::SyncMr) -> String {
    let mut line = String::new();
    if let Some(id) = &mr.id {
        line.push_str(&format!("{id}: "));
    }
    line.push_str(&mr.title);
    line.push_str(&format!(" [branch={}]", mr.branch));
    if let Some(state) = &mr.state {
        line.push_str(&format!(" state={state}"));
    }
    if let Some(url) = &mr.url {
        line.push_str(&format!(" url={url}"));
    }
    if let Some(work_id) = &mr.work_id {
        line.push_str(&format!(" issue={work_id}"));
    }
    line
}

fn collect_ticket_summaries(tickets_dir: &Path) -> Result<Vec<String>> {
    if !tickets_dir.exists() {
        return Ok(vec![]);
    }

    let mut tickets = vec![];
    for entry in fs::read_dir(tickets_dir)
        .with_context(|| format!("reading {}", tickets_dir.display()))?
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let body = fs::read_to_string(&path).unwrap_or_default();
        let title = first_markdown_heading(&body).unwrap_or("(no heading)");
        tickets.push(format!(
            "- {}: {}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            title
        ));
    }
    tickets.sort();
    Ok(tickets)
}

fn collect_pm_repo_state(repo: &Path) -> String {
    let branch = worktree::git(&["rev-parse", "--abbrev-ref", "HEAD"], repo)
        .unwrap_or_else(|e| format!("(unavailable: {:#})", e));
    let dirty = worktree::git(&["status", "--short"], repo)
        .map(|s| if s.is_empty() { "clean".to_string() } else { s })
        .unwrap_or_else(|e| format!("(unavailable: {:#})", e));
    let commits = worktree::git(&["log", "--oneline", "-5"], repo)
        .unwrap_or_else(|e| format!("(unavailable: {:#})", e));

    format!(
        "Current branch: {}\n\nDirty status:\n{}\n\nRecent commits:\n{}",
        branch, dirty, commits
    )
}

fn bounded_lines(lines: &[String], max_items: usize, max_bytes: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut used = 0usize;
    for line in lines.iter().take(max_items) {
        let bounded = utf8_safe_prefix(line, max_bytes);
        let line_bytes = bounded.len().saturating_add(1);
        if used.saturating_add(line_bytes) > max_bytes {
            break;
        }
        out.push(bounded.to_string());
        used = used.saturating_add(line_bytes);
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PmPreflight {
    pub(crate) rendered: String,
    pub(crate) existing_tickets: Vec<String>,
    #[serde(default)]
    pub(crate) source_issues: Vec<String>,
    #[serde(default)]
    pub(crate) open_mrs: Vec<String>,
    #[serde(default)]
    pub(crate) merged_mrs: Vec<String>,
    #[serde(default)]
    pub(crate) open_issues_count: usize,
    #[serde(default)]
    pub(crate) open_mr_count: usize,
    #[serde(default)]
    pub(crate) merged_mr_count: usize,
}

#[derive(Debug, Clone)]
struct ResolvedGuidance {
    path: Option<String>,
    text: String,
}

fn default_schema_version() -> u32 {
    PM_PLAN_JSON_SCHEMA_VERSION
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PmPlanArtifact {
    #[serde(default = "default_schema_version")]
    pub(crate) schema_version: u32,
    pub(crate) profile: String,
    pub(crate) repo: String,
    pub(crate) target: String,
    #[serde(default)]
    pub(crate) open_issue_count: usize,
    #[serde(default)]
    pub(crate) open_mr_count: usize,
    #[serde(default)]
    pub(crate) merged_mr_count: usize,
    pub(crate) ticket_count: usize,
    pub(crate) plan: PmPlan,
}

fn parse_pm_plan(log_text: &str) -> Result<PmPlan> {
    let json = extract_first_json_object(log_text)
        .ok_or_else(|| anyhow::anyhow!("PM planner did not return valid JSON"))?;
    if json.len() > PM_PLAN_JSON_MAX_BYTES {
        anyhow::bail!(
            "PM plan exceeded {} bytes; reject malformed/bounded-overflow plan",
            PM_PLAN_JSON_MAX_BYTES
        );
    }
    let mut plan = serde_json::from_str::<PmPlan>(&json)?;
    for ticket in &mut plan.tickets {
        if ticket.summary.trim().is_empty() {
            ticket.summary = utf8_safe_prefix(&ticket.objective, PM_PLAN_SUMMARY_MAX_BYTES)
                .trim()
                .to_string();
        }
    }
    if plan.title.trim().is_empty() || plan.summary.trim().is_empty() {
        anyhow::bail!("PM plan missing title or summary");
    }
    if plan.title.len() > PM_PLAN_PACKET_TITLE_MAX_BYTES {
        anyhow::bail!("PM plan title too large");
    }
    if plan.summary.len() > PM_PLAN_SUMMARY_MAX_BYTES {
        anyhow::bail!("PM plan summary too large");
    }
    validate_plan(&plan)?;
    Ok(plan)
}

fn validate_plan(plan: &PmPlan) -> Result<()> {
    if plan.tickets.len() > PM_PLAN_MAX_TICKETS {
        anyhow::bail!(
            "PM plan contains {} tickets; max is {}",
            plan.tickets.len(),
            PM_PLAN_MAX_TICKETS
        );
    }

    let mut seen_keys = HashSet::new();
    for packet in &plan.tickets {
        if packet.key.trim().is_empty() {
            anyhow::bail!("work packet missing plan-local key");
        }
        if !seen_keys.insert(packet.key.trim().to_string()) {
            anyhow::bail!("duplicate work packet key '{}' within plan", packet.key);
        }
        validate_packet(packet)?;
    }

    let key_index: HashSet<&str> = seen_keys.iter().map(String::as_str).collect();
    for packet in &plan.tickets {
        if packet.depends_on.len() > PM_PLAN_PACKET_DEPENDENCY_MAX {
            anyhow::bail!(
                "work packet '{}' has {} dependencies; max is {}",
                packet.key,
                packet.depends_on.len(),
                PM_PLAN_PACKET_DEPENDENCY_MAX
            );
        }
        for dep in &packet.depends_on {
            let dep = dep.trim();
            if dep.is_empty() {
                anyhow::bail!("work packet '{}' has an empty dependency key", packet.key);
            }
            if dep.len() > PM_PLAN_PACKET_LIST_ITEM_MAX_BYTES {
                anyhow::bail!(
                    "work packet '{}' has an oversized dependency key",
                    packet.key
                );
            }
            if !key_index.contains(dep.trim()) {
                anyhow::bail!(
                    "work packet '{}' depends on unknown key '{}'",
                    packet.key,
                    dep
                );
            }
        }
    }

    if let Some((left, right)) = detect_overlap_dependency_violations(&plan.tickets) {
        return Err(anyhow::anyhow!(
            "work packet '{}' overlaps non-atomically with '{}'",
            left,
            right
        ));
    }

    let mut remaining: HashMap<String, HashSet<String>> = plan
        .tickets
        .iter()
        .map(|packet| {
            (
                packet.key.trim().to_string(),
                packet
                    .depends_on
                    .iter()
                    .map(|dependency| dependency.trim().to_string())
                    .collect(),
            )
        })
        .collect();
    loop {
        let ready: Vec<String> = remaining
            .iter()
            .filter(|(_, dependencies)| dependencies.is_empty())
            .map(|(key, _)| key.clone())
            .collect();
        if ready.is_empty() {
            break;
        }
        for key in &ready {
            remaining.remove(key);
        }
        for dependencies in remaining.values_mut() {
            for key in &ready {
                dependencies.remove(key);
            }
        }
    }

    if !remaining.is_empty() {
        let mut cyclic_keys: Vec<&str> = remaining.keys().map(String::as_str).collect();
        cyclic_keys.sort_unstable();
        anyhow::bail!(
            "work packet dependency cycle involving: {}",
            cyclic_keys.join(", ")
        );
    }

    Ok(())
}

fn detect_overlap_dependency_violations(packets: &[PlannerWorkPacket]) -> Option<(String, String)> {
    let mut direct_deps: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut all_keys = HashSet::new();

    for packet in packets {
        let key = packet.key.trim();
        all_keys.insert(key);
        let deps: HashSet<&str> = packet.depends_on.iter().map(|d| d.trim()).collect();
        direct_deps.insert(key, deps);
    }

    let mut transitive_depends: HashMap<&str, HashSet<&str>> = HashMap::new();
    for &key in &all_keys {
        let mut visited = HashSet::new();
        let mut queue = vec![key];
        while let Some(current) = queue.pop() {
            if let Some(deps) = direct_deps.get(current) {
                for &dep in deps {
                    if visited.insert(dep) {
                        queue.push(dep);
                    }
                }
            }
        }
        transitive_depends.insert(key, visited);
    }

    let is_dependency_ordered = |key_a: &str, key_b: &str| -> bool {
        transitive_depends
            .get(key_a)
            .is_some_and(|deps| deps.contains(key_b))
            || transitive_depends
                .get(key_b)
                .is_some_and(|deps| deps.contains(key_a))
    };

    for left_index in 0..packets.len() {
        let left = &packets[left_index];
        let left_key = left.key.trim();
        let left_files: HashSet<&str> = left
            .affected_files
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        for right in packets.iter().skip(left_index + 1) {
            let right_key = right.key.trim();
            if right_key == left_key {
                continue;
            }
            let right_files: HashSet<&str> = right
                .affected_files
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();

            let file_overlap = left_files.intersection(&right_files).next().is_some();

            if file_overlap && !is_dependency_ordered(left_key, right_key) {
                return Some((left.key.clone(), right.key.clone()));
            }
        }
    }

    None
}

fn validate_packet(ticket: &PlannerWorkPacket) -> Result<()> {
    if ticket.title.trim().is_empty() {
        anyhow::bail!("work packet missing title");
    }
    if ticket.title.len() > PM_PLAN_PACKET_TITLE_MAX_BYTES {
        anyhow::bail!("work packet '{}' title too large", ticket.title);
    }
    let summary = effective_packet_summary(ticket);
    if summary.trim().is_empty() {
        anyhow::bail!("work packet '{}' missing summary", ticket.title);
    }
    if summary.len() > PM_PLAN_SUMMARY_MAX_BYTES {
        anyhow::bail!("work packet '{}' summary too large", ticket.title);
    }
    if ticket.objective.trim().is_empty() {
        anyhow::bail!("work packet '{}' missing objective", ticket.title);
    }
    if ticket.objective.len() > PM_PLAN_PACKET_OBJECTIVE_MAX_BYTES {
        anyhow::bail!("work packet '{}' objective too large", ticket.title);
    }
    if ticket.key.len() > PM_PLAN_PACKET_TITLE_MAX_BYTES {
        anyhow::bail!("work packet '{}' has oversized key", ticket.title);
    }
    if !matches!(
        ticket.task_class.as_str(),
        "fix" | "feature" | "refactor" | "docs" | "test" | "chore"
    ) {
        anyhow::bail!(
            "work packet '{}' has invalid task_class '{}'",
            ticket.title,
            ticket.task_class
        );
    }
    if !matches!(ticket.difficulty.as_str(), "easy" | "medium" | "hard") {
        anyhow::bail!("work packet '{}' has invalid difficulty", ticket.title);
    }
    if !matches!(ticket.risk.as_str(), "low" | "medium" | "high") {
        anyhow::bail!("work packet '{}' has invalid risk", ticket.title);
    }
    if !matches!(
        ticket.execution_disposition.as_str(),
        "autonomous" | "supervised" | "human_required"
    ) {
        anyhow::bail!(
            "work packet '{}' has invalid execution_disposition",
            ticket.title
        );
    }
    validate_required_bounded_list(
        &ticket.title,
        "affected areas",
        &ticket.affected_areas,
        PM_PLAN_PACKET_MAX_AFFECTED_AREAS,
        PM_PLAN_PACKET_LIST_ITEM_MAX_BYTES,
    )?;
    validate_required_bounded_list(
        &ticket.title,
        "affected files",
        &ticket.affected_files,
        PM_PLAN_PACKET_MAX_AFFECTED_FILES,
        PM_PLAN_PACKET_LIST_ITEM_MAX_BYTES,
    )?;
    validate_required_bounded_list(
        &ticket.title,
        "acceptance criteria",
        &ticket.acceptance_criteria,
        PM_PLAN_PACKET_MAX_CRITERIA,
        PM_PLAN_PACKET_CRITERIA_MAX_BYTES,
    )?;
    validate_required_bounded_list(
        &ticket.title,
        "verification commands",
        &ticket.verification_commands,
        PM_PLAN_PACKET_MAX_VERIFICATION_COMMANDS,
        PM_PLAN_PACKET_VERIFICATION_MAX_BYTES,
    )?;
    validate_optional_bounded_list(
        &ticket.title,
        "duplicate evidence",
        &ticket.duplicate_evidence,
        PM_PLAN_PACKET_DUPLICATE_EVIDENCE_MAX,
        PM_PLAN_PACKET_CRITERIA_MAX_BYTES,
    )?;
    if ticket.uncovered_reason.trim().is_empty() {
        anyhow::bail!("work packet '{}' missing uncovered reason", ticket.title);
    }
    if ticket.uncovered_reason.len() > PM_PLAN_PACKET_UNCOVERED_REASON_MAX_BYTES {
        anyhow::bail!(
            "work packet '{}' has oversized uncovered reason",
            ticket.title
        );
    }
    validate_routing(&ticket.recommended_routing, &ticket.title)?;
    Ok(())
}

fn effective_packet_summary(ticket: &PlannerWorkPacket) -> &str {
    if ticket.summary.trim().is_empty() {
        &ticket.objective
    } else {
        &ticket.summary
    }
}

fn validate_required_bounded_list(
    title: &str,
    field: &str,
    values: &[String],
    max_items: usize,
    max_item_bytes: usize,
) -> Result<()> {
    if values.is_empty() {
        anyhow::bail!("work packet '{}' missing {}", title, field);
    }
    if values.len() > max_items {
        anyhow::bail!(
            "work packet '{}' has {} {}; max is {}",
            title,
            values.len(),
            field,
            max_items
        );
    }
    if values.iter().any(|value| value.trim().is_empty()) {
        anyhow::bail!("work packet '{}' has empty {} entry", title, field);
    }
    if values.iter().any(|value| value.len() > max_item_bytes) {
        anyhow::bail!("work packet '{}' has oversized {} entry", title, field);
    }
    Ok(())
}

fn validate_optional_bounded_list(
    title: &str,
    field: &str,
    values: &[String],
    max_items: usize,
    max_item_bytes: usize,
) -> Result<()> {
    if values.len() > max_items {
        anyhow::bail!(
            "work packet '{}' has {} {}; max is {}",
            title,
            values.len(),
            field,
            max_items
        );
    }
    if values.iter().any(|value| value.trim().is_empty()) {
        anyhow::bail!("work packet '{}' has empty {} entry", title, field);
    }
    if values.iter().any(|value| value.len() > max_item_bytes) {
        anyhow::bail!("work packet '{}' has oversized {} entry", title, field);
    }
    Ok(())
}

fn validate_routing(routing: &RecommendedRouting, title: &str) -> Result<()> {
    if !matches!(
        routing.capability.as_str(),
        "edit" | "plan" | "review" | "research"
    ) {
        anyhow::bail!(
            "work packet '{}' has invalid recommended_routing.capability '{}'",
            title,
            routing.capability
        );
    }
    if !matches!(routing.min_tier.as_str(), "standard" | "strong") {
        anyhow::bail!(
            "work packet '{}' has invalid recommended_routing.min_tier '{}'",
            title,
            routing.min_tier
        );
    }
    Ok(())
}

fn persist_pm_plan_artifact(
    session_dir: &Path,
    profile: &Profile,
    preflight: &PmPreflight,
    target: &str,
    plan: &PmPlan,
) -> Result<PathBuf> {
    let artifact = PmPlanArtifact {
        schema_version: PM_PLAN_JSON_SCHEMA_VERSION,
        profile: profile.display_name.clone(),
        repo: profile.repo.clone(),
        target: utf8_safe_prefix(target, PM_PLAN_PREPARED_TARGET_MAX_BYTES).to_string(),
        open_issue_count: preflight.open_issues_count,
        open_mr_count: preflight.open_mr_count,
        merged_mr_count: preflight.merged_mr_count,
        ticket_count: plan.tickets.len(),
        plan: plan.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&artifact)?;
    if bytes.len() > PM_PLAN_JSON_MAX_BYTES * 2 {
        anyhow::bail!(
            "PM plan artifact exceeds {} bytes; reject malformed plan",
            PM_PLAN_JSON_MAX_BYTES * 2
        );
    }
    let path = session_dir.join("pm-plan-v1.json");
    fs::write(&path, &bytes)?;
    println!("Wrote PM plan artifact: {}", path.display());
    Ok(path)
}

#[cfg(test)]
mod tests;
