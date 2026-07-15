use super::super::attempts::{
    apply_route_to_ledger, decide_route, ensure_bin, mark_backend_unavailable_from_output,
    preflight, record_route_attempt, resolve_llm, route_identity, route_label, run_backend,
};
use super::super::command::command_output;
use super::super::repo_inspection::count_test_files;
use super::super::text::utf8_safe_prefix;
use super::super::text::{extract_first_json_object, first_markdown_heading, normalize_match};
use super::super::DispatchArgs;
use crate::config::{self, GahConfig, Profile};
use crate::ledger::LedgerEntry;
use crate::models::{PlannerWorkPacket, PmPlan, RecommendedRouting};
use crate::routing::RouteRequest;
use crate::worktree;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
pub(crate) fn pm(
    cfg: &GahConfig,
    profile: &Profile,
    args: &DispatchArgs,
    session_dir: &Path,
    ledger: &mut LedgerEntry,
) -> Result<()> {
    let repo = Path::new(&profile.local_path);

    // Without a target: static repo snapshot (context for the agent, not a dispatch)
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
            "# PM Report: {}\n\nRepo: {}\nBranch: {}\nTest files: {}\nCI configured: {}\n\n\
             ## Recent commits\n```\n{}\n```\n\n## README\n{}\n",
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

    // With a target: dispatch an LLM to produce a structured ticket plan.
    let preflight_ctx = collect_pm_preflight(profile, repo)?;
    let route_req = RouteRequest {
        mode: "pm",
        requested_backend: config::canonical_backend_name(&args.backend),
        requested_model: args.model.as_deref(),
        recommended_backend: None,
        recommended_model: None,
        session_id: session_dir.file_name().and_then(|s| s.to_str()),
        usage_summary: None,
        last_failure_class: None,
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

    // Resolve env_file: use env_file_prod if --prod, otherwise env_file (dev)
    let resolved_env = if args.prod {
        profile.env_file_prod.as_deref().unwrap_or("")
    } else {
        profile.env_file.as_deref().unwrap_or("")
    };
    if !resolved_env.is_empty() {
        println!("Env file: {}", resolved_env);
        if args.prod {
            println!("  \u{26a0}\u{fe0f}  PRODUCTION env - agent has live API access");
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
    let written = apply_pm_plan(repo, &preflight_ctx, &plan)?;
    println!("\nCreated {} ticket(s):", written.len());
    for path in &written {
        println!("  {}", path.display());
    }

    Ok(())
}

fn build_pm_plan_task(profile: &Profile, ctx: &PmPreflight, target: &str) -> Result<String> {
    Ok(format!(
        "Repository: {} ({})\nLocal path: {}\nTarget branch: {}\n\n\
         You are a project manager. Use only the preflight context below plus the target request. \
         Do not assume you will remember to inspect the repo yourself.\n\
         Do not write code in PM mode.\n\
         Return only valid JSON matching this bounded schema:\n\
         {{\"title\":string,\"summary\":string,\"tickets\":[{{\n  \
         \"key\":string (plan-local stable key, unique within the plan),\n  \
         \"title\":string,\n  \
         \"objective\":string (the why, distict from title),\n  \
         \"task_class\":\"fix|feature|refactor|docs|test|chore\",\n  \
         \"difficulty\":\"easy|medium|hard\",\n  \
         \"risk\":\"low|medium|high\",\n  \
         \"execution_disposition\":\"autonomous|supervised|human_required\",\n  \
         \"recommended_routing\":{{\"capability\":\"edit|plan|review|research\",\"min_tier\":\"standard|strong\"}},\n  \
         \"affected_areas\":[string],\n  \
         \"affected_files\":[string],\n  \
         \"acceptance_criteria\":[string],\n  \
         \"verification_commands\":[string],\n  \
         \"depends_on\":[string] (plan-local keys of prerequisite packets),\n  \
         \"duplicate_evidence\":[string],\n  \
         \"uncovered_reason\":string\n}}]}}\n\
         Recommended routing MUST express a capability and difficulty tier, never a literal model name.\n\n\
         Rules:\n\
         - Default action is to avoid creating new tickets.\n\
         - Do not create a ticket if an open MR already covers the issue.\n\
         - Do not create a ticket if an existing ticket already covers the issue.\n\
         - Do not create a ticket if a recently merged MR already fixed the issue.\n\
         - Consolidate small related fixes into one ticket.\n\
         - Prefer \"already covered\" or \"update existing ticket\" when unsure.\n\
         - If creating tickets, create only genuinely uncovered, atomic work.\n\
         - Cite the relevant MR or ticket evidence whenever you decide work is already covered.\n\
         - If the work is already covered, return an empty tickets array.\n\
         - Each new ticket must be independently completable in one session.\n\
         - Every `key` must be unique within the plan; `depends_on` references only keys present in the plan.\n\n\
         ## Preflight Context\n\n{}\n\n\
         ## Target Request\n\n{}\n",
        profile.display_name,
        profile.repo,
        profile.local_path,
        profile.default_target_branch,
        ctx.rendered,
        target,
    ))
}

fn collect_pm_preflight(profile: &Profile, repo: &Path) -> Result<PmPreflight> {
    let memory_path = repo.join("docs/MANAGER_MEMORY.md");
    let manager_memory = fs::read_to_string(&memory_path).with_context(|| {
        format!(
            "PM mode requires manager memory at {}",
            memory_path.display()
        )
    })?;

    let repo_state = collect_pm_repo_state(repo);
    let tickets = collect_ticket_summaries(&repo.join("docs/tickets"))?;
    let open_mrs = collect_mr_context(profile, repo, "opened", None)?;
    let merged_mrs = collect_mr_context(profile, repo, "merged", Some("20"))?;

    let mut out = String::new();
    out.push_str("### Manager Memory\n");
    out.push_str(&manager_memory);
    if !manager_memory.ends_with('\n') {
        out.push('\n');
    }

    out.push_str("\n### Open Merge Requests\n");
    out.push_str(&open_mrs);
    out.push_str("\n\n### Recently Merged Merge Requests\n");
    out.push_str(&merged_mrs);
    out.push_str("\n\n### Existing Tickets\n");
    if tickets.is_empty() {
        out.push_str("(none found)");
    } else {
        out.push_str(&tickets.join("\n"));
    }
    out.push_str("\n\n### Repo State\n");
    out.push_str(&repo_state);
    Ok(PmPreflight {
        rendered: out,
        existing_tickets: tickets,
        open_mrs,
        merged_mrs,
    })
}

fn collect_mr_context(
    profile: &Profile,
    repo: &Path,
    state: &str,
    per_page: Option<&str>,
) -> Result<String> {
    if profile.provider != "gitlab" {
        return Ok("(provider is not gitlab)".to_string());
    }

    ensure_bin("glab")?;
    let mut args = vec![
        "mr",
        "list",
        "--repo",
        profile.repo.as_str(),
        "--state",
        state,
    ];
    if let Some(per_page) = per_page {
        args.push("--per-page");
        args.push(per_page);
    }
    let output = command_output("glab", &args, repo)?;
    if output.is_empty() {
        Ok("(none)".to_string())
    } else {
        Ok(output)
    }
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
    let branch = command_output("git", &["rev-parse", "--abbrev-ref", "HEAD"], repo)
        .unwrap_or_else(|e| format!("(unavailable: {:#})", e));
    let dirty = command_output("git", &["status", "--short"], repo)
        .map(|s| if s.is_empty() { "clean".to_string() } else { s })
        .unwrap_or_else(|e| format!("(unavailable: {:#})", e));
    let commits = command_output("git", &["log", "--oneline", "-5"], repo)
        .unwrap_or_else(|e| format!("(unavailable: {:#})", e));

    format!(
        "Current branch: {}\n\nDirty status:\n{}\n\nRecent commits:\n{}",
        branch, dirty, commits
    )
}

#[derive(Debug, Clone)]
struct PmPreflight {
    rendered: String,
    existing_tickets: Vec<String>,
    open_mrs: String,
    merged_mrs: String,
}

fn parse_pm_plan(log_text: &str) -> Result<PmPlan> {
    let json = extract_first_json_object(log_text)
        .ok_or_else(|| anyhow::anyhow!("PM planner did not return valid JSON"))?;
    let plan = serde_json::from_str::<PmPlan>(&json)?;
    if plan.title.trim().is_empty() || plan.summary.trim().is_empty() {
        anyhow::bail!("PM plan missing title or summary");
    }
    validate_plan(&plan)?;
    Ok(plan)
}

/// TICKET-544: validate the bounded plan schema end-to-end before any ticket
/// is written. This is the authoritative check that the planner produced a
/// coherent, provider-neutral work packet set.
fn validate_plan(plan: &PmPlan) -> Result<()> {
    let mut seen_keys = std::collections::HashSet::new();
    for packet in &plan.tickets {
        if packet.key.trim().is_empty() {
            anyhow::bail!("work packet missing plan-local key");
        }
        if !seen_keys.insert(packet.key.trim().to_string()) {
            anyhow::bail!("duplicate work packet key '{}' within plan", packet.key);
        }
        validate_packet(packet)?;
    }
    let keys: std::collections::HashSet<&str> = plan.tickets.iter().map(|p| p.key.trim()).collect();
    for packet in &plan.tickets {
        for dep in &packet.depends_on {
            if !keys.contains(dep.as_str()) {
                anyhow::bail!(
                    "work packet '{}' depends on unknown key '{}'",
                    packet.key,
                    dep
                );
            }
        }
    }
    Ok(())
}

fn apply_pm_plan(repo: &Path, ctx: &PmPreflight, plan: &PmPlan) -> Result<Vec<PathBuf>> {
    let tickets_dir = repo.join("docs/tickets");
    fs::create_dir_all(&tickets_dir)?;
    let manager_memory_path = repo.join("docs/MANAGER_MEMORY.md");
    let next_id = next_ticket_id(&tickets_dir, Some(&manager_memory_path))?;
    let mut written = vec![];
    let mut id = next_id;
    for ticket in &plan.tickets {
        if should_skip_ticket(ctx, ticket) {
            continue;
        }
        validate_packet(ticket)?;
        let slug = slugify(&ticket.title);
        let filename = format!("TICKET-{:03}-{}.md", id, slug);
        let path = tickets_dir.join(filename);
        fs::write(&path, render_ticket(ticket, id))?;
        written.push(path);
        id += 1;
    }
    Ok(written)
}

fn should_skip_ticket(ctx: &PmPreflight, ticket: &PlannerWorkPacket) -> bool {
    let title = normalize_match(&ticket.title);
    if title.is_empty() {
        return true;
    }
    ctx.existing_tickets
        .iter()
        .any(|item| normalize_match(item).contains(&title))
        || normalize_match(&ctx.open_mrs).contains(&title)
        || normalize_match(&ctx.merged_mrs).contains(&title)
}

fn validate_packet(ticket: &PlannerWorkPacket) -> Result<()> {
    if ticket.title.trim().is_empty() || ticket.objective.trim().is_empty() {
        anyhow::bail!("work packet missing title or objective");
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
    validate_routing(&ticket.recommended_routing, &ticket.title)?;
    if ticket.acceptance_criteria.is_empty() || ticket.verification_commands.is_empty() {
        anyhow::bail!(
            "work packet '{}' missing acceptance or verification",
            ticket.title
        );
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

fn render_ticket(ticket: &PlannerWorkPacket, id: usize) -> String {
    let mut out = format!(
        "# TICKET-{id:03}: {title}\n\n\
        Plan key: {key}\n\
        Objective: {objective}\n\n\
        Task class: {task_class}\n\
        Difficulty: {difficulty}\n\
        Risk: {risk}\n\
        Execution disposition: {disposition}\n\
        Recommended routing: capability={capability} min_tier={tier}\n\n\
        ## Why This Is Uncovered\n{reason}\n\n\
        ## Affected Areas\n",
        id = id,
        title = ticket.title,
        key = ticket.key,
        objective = ticket.objective,
        task_class = ticket.task_class,
        difficulty = ticket.difficulty,
        risk = ticket.risk,
        disposition = ticket.execution_disposition,
        capability = ticket.recommended_routing.capability,
        tier = ticket.recommended_routing.min_tier,
        reason = ticket.uncovered_reason,
    );
    if ticket.affected_areas.is_empty() {
        out.push_str("(none specified)\n");
    }
    for area in &ticket.affected_areas {
        out.push_str(&format!("- {}\n", area));
    }
    out.push_str("\n## Affected Files\n");
    if ticket.affected_files.is_empty() {
        out.push_str("(none specified)\n");
    }
    for file in &ticket.affected_files {
        out.push_str(&format!("- {}\n", file));
    }
    if !ticket.depends_on.is_empty() {
        out.push_str("\n## Depends On\n");
        for dep in &ticket.depends_on {
            out.push_str(&format!("- {}\n", dep));
        }
    }
    if !ticket.duplicate_evidence.is_empty() {
        out.push_str("\n## Duplicate Evidence Considered\n");
        for item in &ticket.duplicate_evidence {
            out.push_str(&format!("- {}\n", item));
        }
    }
    out.push_str("\n## Acceptance Criteria\n");
    for item in &ticket.acceptance_criteria {
        out.push_str(&format!("- {}\n", item));
    }
    out.push_str("\n## Verification Commands\n");
    for cmd in &ticket.verification_commands {
        out.push_str(&format!("- `{}`\n", cmd));
    }
    out
}

/// TICKET-091 AC6/7: a new ticket ID must not collide with one already
/// reserved in manager memory prose, even if no file exists yet for it
/// (this is exactly how the TICKET-102/103/104 collisions happened).
fn next_ticket_id(tickets_dir: &Path, manager_memory_path: Option<&Path>) -> Result<usize> {
    let mut max_id = 0usize;
    if tickets_dir.exists() {
        for entry in fs::read_dir(tickets_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(rest) = name.strip_prefix("TICKET-") {
                if let Some((num, _)) = rest.split_once('-') {
                    max_id = max_id.max(num.parse::<usize>().unwrap_or(0));
                }
            }
        }
    }
    if let Some(path) = manager_memory_path {
        if path.exists() {
            let content = fs::read_to_string(path)?;
            for id in scan_ticket_ids(&content) {
                max_id = max_id.max(id);
            }
        }
    }
    Ok(max_id + 1)
}

fn scan_ticket_ids(text: &str) -> Vec<usize> {
    text.split("TICKET-")
        .skip(1)
        .filter_map(|rest| {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<usize>().ok()
        })
        .collect()
}

fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests;
