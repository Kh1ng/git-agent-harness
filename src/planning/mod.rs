//! `gah map`: an epic's issues, or a chartr `.plan/maps/` map, as a
//! read-only planning map. Nothing here writes to the provider or a file.
//!
//! - `map` decides nodes, edges, and the frontier from issue facts.
//! - `fetch` reads those facts from GitHub or GitLab.
//! - `files` reads chartr map files from the checkout.

pub mod fetch;
pub mod files;
pub mod map;

use anyhow::{bail, Result};
use serde::Serialize;

#[derive(clap::Args)]
pub struct Args {
    #[arg(long)]
    profile: String,
    /// The epic to map. Omit both --epic and --file to list what can be mapped.
    #[arg(long, conflicts_with = "file")]
    epic: Option<u64>,
    /// A chartr map in the checkout's `.plan/maps/<slug>/`, read from files
    /// only (no provider requests).
    #[arg(long)]
    file: Option<String>,
    #[arg(long)]
    json: bool,
    #[arg(long = "config", visible_alias = "config-path")]
    config_path: Option<String>,
}

#[derive(Serialize)]
struct EpicList {
    epics: Vec<map::EpicSummary>,
    /// Chartr map files in the checkout.
    files: Vec<files::MapFileSummary>,
    /// Why the issues could not be listed; the map files still are.
    #[serde(skip_serializing_if = "Option::is_none")]
    issues_error: Option<String>,
}

pub fn run(args: Args) -> Result<()> {
    let config = crate::config::load(args.config_path.as_deref())?;
    let profile = crate::config::get_profile(&config, &args.profile)?;
    let checkout = std::path::Path::new(&profile.local_path);
    if let Some(slug) = &args.file {
        let plan = files::load(checkout, slug)?;
        return print_map(&plan, args.json);
    }
    let Some(epic) = args.epic else {
        return print_list(profile, checkout, args.json);
    };
    let mut listing = fetch::listing(profile)?;
    if !listing.issues.contains_key(&epic) {
        bail!("issue #{epic} is not in {}", profile.repo);
    }
    fetch::relate(profile, epic, &mut listing)?;
    print_map(&map::build(epic, &listing.issues), args.json)
}

fn print_list(
    profile: &crate::config::Profile,
    checkout: &std::path::Path,
    json: bool,
) -> Result<()> {
    let files = files::list(checkout)?;
    let (epics, issues_error) = match fetch::listing(profile) {
        Ok(listing) => (
            map::epics(&listing.issues, &listing.native_child_counts),
            None,
        ),
        // Map files need no provider; report the issues problem beside them.
        Err(error) if !files.is_empty() => (
            Vec::new(),
            Some(crate::redact::redact(&format!("{error:#}"))),
        ),
        Err(error) => return Err(error),
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&EpicList {
                epics,
                files,
                issues_error
            })?
        );
        return Ok(());
    }
    if let Some(error) = &issues_error {
        eprintln!("Issues could not be read: {error}");
    }
    if epics.is_empty() && files.is_empty() {
        println!(
            "No epics: no issue has children or an `epic` label, and there is no .plan/maps/ map."
        );
    }
    for epic in epics {
        println!(
            "#{} {}{}: {} of {} children open",
            epic.number,
            epic.title,
            if epic.open { "" } else { " (closed)" },
            epic.open_children,
            epic.children
        );
    }
    for file in files {
        println!(
            "{} (.plan/maps): {}: {} of {} tickets open",
            file.slug, file.title, file.open_tickets, file.tickets
        );
    }
    Ok(())
}

fn print_map(plan: &map::PlanMap, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(plan)?);
        return Ok(());
    }
    let mut nodes: Vec<&map::Node> = plan.nodes.iter().collect();
    nodes.sort_by_key(|node| (node.depth.unwrap_or(usize::MAX), node.number));
    for node in nodes {
        let indent = "  ".repeat(node.depth.unwrap_or(1));
        let state = match node.state {
            map::NodeState::Done => "done",
            map::NodeState::Ready => "ready",
            map::NodeState::Blocked => "blocked",
            map::NodeState::Parent => "parent",
            map::NodeState::RuledOut => "ruled out",
            map::NodeState::Claimed => "claimed",
        };
        let waiting = if node.waiting_on.is_empty() {
            String::new()
        } else {
            let numbers: Vec<String> = node.waiting_on.iter().map(|n| format!("#{n}")).collect();
            format!(", waiting on {}", numbers.join(" "))
        };
        let outside = if node.depth.is_none() {
            ", outside the epic"
        } else {
            ""
        };
        println!(
            "{indent}#{} {} [{state}{waiting}{outside}]",
            node.number, node.title
        );
    }
    let frontier: Vec<String> = plan.frontier.iter().map(|n| format!("#{n}")).collect();
    println!(
        "Frontier: {}",
        if frontier.is_empty() {
            "none".to_string()
        } else {
            frontier.join(" ")
        }
    );
    for problem in &plan.diagnostics {
        eprintln!("Skipped: {problem}");
    }
    Ok(())
}
