//! `gah map`: an epic's issues as a read-only planning map. Nothing here
//! writes to the provider.
//!
//! - `map` decides nodes, edges, and the frontier from issue facts.
//! - `fetch` reads those facts from GitHub or GitLab.

pub mod fetch;
pub mod map;

use anyhow::{bail, Result};
use serde::Serialize;

#[derive(clap::Args)]
pub struct Args {
    #[arg(long)]
    profile: String,
    /// The epic to map. Omit to list the issues that can be mapped.
    #[arg(long)]
    epic: Option<u64>,
    #[arg(long)]
    json: bool,
    #[arg(long = "config", visible_alias = "config-path")]
    config_path: Option<String>,
}

#[derive(Serialize)]
struct EpicList {
    epics: Vec<map::EpicSummary>,
}

pub fn run(args: Args) -> Result<()> {
    let config = crate::config::load(args.config_path.as_deref())?;
    let profile = crate::config::get_profile(&config, &args.profile)?;
    let mut listing = fetch::listing(profile)?;
    let Some(epic) = args.epic else {
        let epics = map::epics(&listing.issues, &listing.native_child_counts);
        if args.json {
            println!("{}", serde_json::to_string_pretty(&EpicList { epics })?);
        } else if epics.is_empty() {
            println!("No epics: no issue has children or an `epic` label.");
        } else {
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
        }
        return Ok(());
    };
    if !listing.issues.contains_key(&epic) {
        bail!("issue #{epic} is not in {}", profile.repo);
    }
    fetch::relate(profile, epic, &mut listing)?;
    let plan = map::build(epic, &listing.issues);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
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
    Ok(())
}
