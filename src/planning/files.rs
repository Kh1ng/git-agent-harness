//! Chartr map files (`.plan/maps/<slug>/`) as an optional, read-only source
//! for the planning map (#1241, #799). Issues stay authoritative: nothing
//! here writes a file or an issue.
//!
//! Format, from chartr's write contract (docs/CHARTR_MAP_RESEARCH_2026-09-09.md):
//! `map.md` titles the destination; `tickets/NN-slug.md` carries a small
//! frontmatter (`type`, `blocked_by`, `claimed_by`) and `## Answer` or
//! `## Ruled out` sections that decide its status.

use super::map::{Edge, EdgeKind, Node, NodeState, PlanMap};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Files past these limits are reported, not read: a map is a planning
/// document, not a data store.
const MAX_TICKETS: usize = 500;
const MAX_FILE_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Open,
    Claimed,
    Resolved,
    OutOfScope,
}

#[derive(Debug, Clone)]
pub struct Ticket {
    pub number: u64,
    pub title: String,
    pub kind: String,
    pub blocked_by: Vec<u64>,
    pub status: Status,
    /// Repository-relative path.
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MapFileSummary {
    pub slug: String,
    pub title: String,
    pub tickets: usize,
    /// Tickets neither resolved nor ruled out.
    pub open_tickets: usize,
}

/// A map slug as chartr writes it: lower-kebab, so it can never name a path
/// outside `.plan/maps/`.
pub fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 100
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-')
}

/// Lines outside fenced code blocks, so an example ticket inside a fence
/// cannot change this ticket's status.
fn unfenced(body: &str) -> Vec<&str> {
    let mut fence: Option<&str> = None;
    let mut lines = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim_start();
        let marker = if trimmed.starts_with("```") {
            Some("```")
        } else if trimmed.starts_with("~~~") {
            Some("~~~")
        } else {
            None
        };
        match (fence, marker) {
            (None, Some(open)) => fence = Some(open),
            (Some(open), Some(close)) if open == close => fence = None,
            (None, None) => lines.push(line),
            _ => {}
        }
    }
    lines
}

/// The text under an exact `## heading`, up to the next `#`/`##` heading.
fn section(lines: &[&str], heading: &str) -> String {
    let mut inside = false;
    let mut text = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("# ") || trimmed.starts_with("## ") {
            inside = trimmed
                .strip_prefix("## ")
                .is_some_and(|h| h.trim() == heading);
            continue;
        }
        if inside {
            text.push(trimmed);
        }
    }
    text.join("\n").trim().to_string()
}

fn first_heading(lines: &[&str]) -> Option<String> {
    lines
        .iter()
        .find_map(|line| line.trim().strip_prefix("# "))
        .map(|title| title.trim().to_string())
}

/// Splits leading `---` frontmatter into `key: value` pairs (single-line
/// keys and inline lists only, as chartr reads them) and the body.
fn frontmatter(text: &str) -> (BTreeMap<String, String>, &str) {
    let mut fields = BTreeMap::new();
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (fields, text);
    };
    let Some(end) = rest.find("\n---") else {
        return (fields, text);
    };
    for line in rest[..end].lines() {
        if let Some((key, value)) = line.split_once(':') {
            fields.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    let after = &rest[end + 4..];
    (fields, after.split_once('\n').map_or("", |(_, body)| body))
}

/// `[01, 02]` or `01, 02` to ticket numbers; anything that is not a number
/// is returned as an error so the ticket can be reported.
fn id_list(value: &str) -> Result<Vec<u64>, String> {
    let inner = value.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            let digits: String = item.chars().take_while(char::is_ascii_digit).collect();
            digits
                .parse()
                .map_err(|_| format!("`{item}` is not a ticket number"))
        })
        .collect()
}

/// Ticket number from `NN-slug.md`.
pub fn ticket_number(file_name: &str) -> Option<u64> {
    let stem = file_name.strip_suffix(".md")?;
    let (number, _) = stem.split_once('-').unwrap_or((stem, ""));
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    number.parse().ok()
}

pub fn parse_ticket(number: u64, path: &str, text: &str) -> Result<Ticket, String> {
    let (fields, body) = frontmatter(text);
    let lines = unfenced(body);
    let blocked_by = match fields.get("blocked_by") {
        Some(value) => id_list(value)?,
        None => Vec::new(),
    };
    let status = if !section(&lines, "Answer").is_empty() {
        Status::Resolved
    } else if !section(&lines, "Ruled out").is_empty() {
        Status::OutOfScope
    } else if fields.get("claimed_by").is_some_and(|who| !who.is_empty()) {
        Status::Claimed
    } else {
        Status::Open
    };
    Ok(Ticket {
        number,
        title: first_heading(&lines).unwrap_or_else(|| format!("Ticket {number:02}")),
        kind: fields.get("type").cloned().unwrap_or_default(),
        blocked_by,
        status,
        path: path.to_string(),
    })
}

/// Each ticket's ring: 1 with no blockers in the map, otherwise one past its
/// deepest blocker, so a blocker sits inside what it blocks (chartr's
/// dependency-depth rings). A cycle stops growing after one pass per ticket.
fn dependency_depths(tickets: &[Ticket]) -> BTreeMap<u64, usize> {
    let mut depth: BTreeMap<u64, usize> = tickets.iter().map(|t| (t.number, 1)).collect();
    for _ in 0..tickets.len() {
        let mut changed = false;
        for ticket in tickets {
            let deepest = ticket
                .blocked_by
                .iter()
                .filter_map(|blocker| depth.get(blocker))
                .max()
                .copied();
            if let Some(deepest) = deepest {
                let wanted = (deepest + 1).min(tickets.len());
                if depth[&ticket.number] < wanted {
                    depth.insert(ticket.number, wanted);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    depth
}

/// The map from parsed tickets. The destination is node 0 at the centre and
/// every ticket hangs from it; blocker edges carry the dependencies. The
/// frontier follows chartr: open tickets whose blockers all exist and are
/// resolved (ruled out does not unblock).
pub fn build(slug: &str, title: &str, tickets: &[Ticket], diagnostics: Vec<String>) -> PlanMap {
    let by_number: BTreeMap<u64, &Ticket> = tickets.iter().map(|t| (t.number, t)).collect();
    let depths = dependency_depths(tickets);
    let resolved = |number: &u64| {
        by_number
            .get(number)
            .is_some_and(|t| t.status == Status::Resolved)
    };
    let mut edges = Vec::new();
    let mut missing = std::collections::BTreeSet::new();
    let mut nodes = Vec::new();
    for ticket in tickets {
        // The centre links only the first ring; deeper tickets hang from
        // their blockers, so lines never cut across the map.
        if depths.get(&ticket.number) == Some(&1) {
            edges.push(Edge {
                from: 0,
                to: ticket.number,
                kind: EdgeKind::Child,
            });
        }
        for blocker in &ticket.blocked_by {
            if !by_number.contains_key(blocker) {
                missing.insert(*blocker);
            }
            edges.push(Edge {
                from: *blocker,
                to: ticket.number,
                kind: EdgeKind::Blocks,
            });
        }
        let waiting_on: Vec<u64> = ticket
            .blocked_by
            .iter()
            .copied()
            .filter(|b| !resolved(b))
            .collect();
        let state = match ticket.status {
            Status::Resolved => NodeState::Done,
            Status::OutOfScope => NodeState::RuledOut,
            Status::Claimed => NodeState::Claimed,
            Status::Open if waiting_on.is_empty() => NodeState::Ready,
            Status::Open => NodeState::Blocked,
        };
        nodes.push(Node {
            number: ticket.number,
            title: ticket.title.clone(),
            url: String::new(),
            labels: if ticket.kind.is_empty() {
                Vec::new()
            } else {
                vec![ticket.kind.clone()]
            },
            state,
            depth: depths.get(&ticket.number).copied(),
            waiting_on,
            path: Some(ticket.path.clone()),
        });
    }
    let finished = !tickets.is_empty()
        && tickets
            .iter()
            .all(|t| matches!(t.status, Status::Resolved | Status::OutOfScope));
    nodes.insert(
        0,
        Node {
            number: 0,
            title: title.to_string(),
            url: String::new(),
            labels: Vec::new(),
            state: if finished {
                NodeState::Done
            } else {
                NodeState::Parent
            },
            depth: Some(0),
            waiting_on: Vec::new(),
            path: Some(format!(".plan/maps/{slug}/map.md")),
        },
    );
    edges.sort();
    edges.dedup();
    let frontier = nodes
        .iter()
        .filter(|node| node.number != 0 && node.state == NodeState::Ready)
        .map(|node| node.number)
        .collect();
    PlanMap {
        epic: 0,
        nodes,
        edges,
        frontier,
        missing: missing.into_iter().collect(),
        file: Some(slug.to_string()),
        diagnostics,
    }
}

fn read_limited(path: &Path) -> Result<String> {
    let size = std::fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    if size > MAX_FILE_BYTES {
        bail!(
            "{} is larger than {} KB",
            path.display(),
            MAX_FILE_BYTES / 1024
        );
    }
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

/// `.plan/maps` under the checkout, refusing a symlink that leads outside it.
fn maps_dir(checkout: &Path) -> Result<Option<PathBuf>> {
    let dir = checkout.join(".plan/maps");
    if !dir.is_dir() {
        return Ok(None);
    }
    let root = std::fs::canonicalize(checkout)?;
    let resolved = std::fs::canonicalize(&dir)?;
    if !resolved.starts_with(&root) {
        bail!(".plan/maps points outside the checkout");
    }
    Ok(Some(resolved))
}

fn map_title(dir: &Path, slug: &str) -> String {
    read_limited(&dir.join("map.md"))
        .ok()
        .and_then(|text| first_heading(&unfenced(&text)))
        .unwrap_or_else(|| slug.to_string())
}

/// Every map under `.plan/maps/` in the checkout; empty when there is none.
pub fn list(checkout: &Path) -> Result<Vec<MapFileSummary>> {
    let Some(maps) = maps_dir(checkout)? else {
        return Ok(Vec::new());
    };
    let mut summaries = Vec::new();
    for entry in std::fs::read_dir(&maps)?.flatten() {
        let slug = entry.file_name().to_string_lossy().into_owned();
        if !valid_slug(&slug) || !entry.path().join("map.md").is_file() {
            continue;
        }
        let map = load(checkout, &slug)?;
        summaries.push(MapFileSummary {
            slug,
            title: map
                .nodes
                .first()
                .map(|n| n.title.clone())
                .unwrap_or_default(),
            tickets: map.nodes.len() - 1,
            open_tickets: map
                .nodes
                .iter()
                .skip(1)
                .filter(|n| !matches!(n.state, NodeState::Done | NodeState::RuledOut))
                .count(),
        });
    }
    summaries.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(summaries)
}

/// One map, read from `.plan/maps/<slug>/`. Unreadable or malformed tickets
/// are left out and named in `diagnostics`; the rest of the map still loads.
pub fn load(checkout: &Path, slug: &str) -> Result<PlanMap> {
    if !valid_slug(slug) {
        bail!("`{slug}` is not a map name (lower-case letters, digits, and dashes)");
    }
    let Some(maps) = maps_dir(checkout)? else {
        bail!("this checkout has no .plan/maps directory");
    };
    let dir = maps.join(slug);
    if !dir.join("map.md").is_file() {
        bail!("no map named `{slug}` in .plan/maps");
    }
    let title = map_title(&dir, slug);
    let mut diagnostics = Vec::new();
    let mut tickets = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join("tickets"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    files.sort();
    if files.len() > MAX_TICKETS {
        diagnostics.push(format!(
            "only the first {MAX_TICKETS} of {} tickets are shown",
            files.len()
        ));
        files.truncate(MAX_TICKETS);
    }
    let mut seen = BTreeMap::new();
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let relative = format!(".plan/maps/{slug}/tickets/{name}");
        let Some(number) = ticket_number(&name) else {
            diagnostics.push(format!(
                "{relative}: the file name does not start with a ticket number"
            ));
            continue;
        };
        if let Some(first) = seen.insert(number, relative.clone()) {
            diagnostics.push(format!(
                "{relative}: ticket {number:02} is also {first}; the later file is skipped"
            ));
            continue;
        }
        match read_limited(&path)
            .map_err(|error| error.to_string())
            .and_then(|text| parse_ticket(number, &relative, &text))
        {
            Ok(ticket) => tickets.push(ticket),
            Err(error) => diagnostics.push(format!("{relative}: {error}")),
        }
    }
    Ok(build(slug, &title, &tickets, diagnostics))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(number: u64, blocked_by: &str, body: &str) -> String {
        format!("---\ntype: task\nblocked_by: [{blocked_by}]\n---\n# Ticket {number}\n\n{body}\n")
    }

    fn parsed(number: u64, text: &str) -> Ticket {
        parse_ticket(number, "t.md", text).unwrap()
    }

    #[test]
    fn status_follows_chartr_precedence() {
        assert_eq!(
            parsed(1, &ticket(1, "", "## Answer\nYes.")).status,
            Status::Resolved
        );
        assert_eq!(
            parsed(1, &ticket(1, "", "## Ruled out\nNot now.")).status,
            Status::OutOfScope
        );
        let claimed = "---\nclaimed_by: ana\n---\n# T\n";
        assert_eq!(parsed(1, claimed).status, Status::Claimed);
        let answered_and_claimed = "---\nclaimed_by: ana\n---\n# T\n## Answer\nDone.\n";
        assert_eq!(
            parsed(1, answered_and_claimed).status,
            Status::Resolved,
            "closure wins over a leftover claim"
        );
        assert_eq!(
            parsed(1, &ticket(1, "", "## Answer\n")).status,
            Status::Open,
            "a bare heading closes nothing"
        );
        assert_eq!(
            parsed(1, &ticket(1, "", "## Proposed Answer\nMaybe.")).status,
            Status::Open
        );
    }

    #[test]
    fn fenced_examples_do_not_change_status_or_title() {
        let text = "---\nblocked_by: []\n---\n# Real title\n\n```markdown\n# Fake\n## Answer\nfenced\n```\n";
        let ticket = parsed(3, text);
        assert_eq!(ticket.status, Status::Open);
        assert_eq!(ticket.title, "Real title");
    }

    #[test]
    fn blockers_parse_and_reject_non_numbers() {
        assert_eq!(parsed(2, &ticket(2, "01, 03", "")).blocked_by, vec![1, 3]);
        assert!(parse_ticket(2, "t.md", &ticket(2, "first", "")).is_err());
        assert_eq!(ticket_number("02-implement-transfer.md"), Some(2));
        assert_eq!(ticket_number("notes.md"), None);
        assert!(
            valid_slug("node-handoff")
                && !valid_slug("../x")
                && !valid_slug("A")
                && !valid_slug("")
        );
    }

    #[test]
    fn the_frontier_needs_every_blocker_resolved() {
        let tickets = [
            parsed(1, &ticket(1, "", "## Answer\nCommitted changes only.")),
            parsed(2, &ticket(2, "01", "")),
            parsed(3, &ticket(3, "04", "")),
            parsed(4, &ticket(4, "", "## Ruled out\nNo.")),
            parsed(5, &ticket(5, "09", "")),
            parsed(6, &ticket(6, "02", "")),
        ];
        let map = build("handoff", "Node handoff", &tickets, Vec::new());
        let state = |n: u64| {
            map.nodes
                .iter()
                .find(|node| node.number == n)
                .unwrap()
                .state
        };
        assert_eq!(map.frontier, vec![2]);
        assert_eq!(state(1), NodeState::Done);
        assert_eq!(
            state(3),
            NodeState::Blocked,
            "a ruled-out blocker does not unblock"
        );
        assert_eq!(state(4), NodeState::RuledOut);
        assert_eq!(
            state(5),
            NodeState::Blocked,
            "a missing blocker does not unblock"
        );
        assert_eq!(state(6), NodeState::Blocked);
        assert_eq!(map.missing, vec![9]);
        assert_eq!(map.file.as_deref(), Some("handoff"));
        assert_eq!(map.nodes[0].title, "Node handoff");
        assert_eq!(state(0), NodeState::Parent);
        assert!(map.edges.contains(&Edge {
            from: 1,
            to: 2,
            kind: EdgeKind::Blocks
        }));
        let done = build(
            "x",
            "X",
            &[parsed(1, &ticket(1, "", "## Answer\nYes."))],
            Vec::new(),
        );
        assert_eq!(
            done.nodes[0].state,
            NodeState::Done,
            "a map with every ticket closed is finished"
        );
    }

    #[test]
    fn tickets_sit_one_ring_past_their_deepest_blocker_and_cycles_stop() {
        let tickets = [
            parsed(1, &ticket(1, "", "")),
            parsed(2, &ticket(2, "01", "")),
            parsed(3, &ticket(3, "01, 02", "")),
            parsed(4, &ticket(4, "05", "")),
            parsed(5, &ticket(5, "04", "")),
        ];
        let depths = dependency_depths(&tickets);
        assert_eq!((depths[&1], depths[&2], depths[&3]), (1, 2, 3));
        assert!(
            depths[&4] <= tickets.len() && depths[&5] <= tickets.len(),
            "a cycle is bounded"
        );
    }

    #[test]
    fn maps_load_from_the_checkout_and_report_bad_tickets() {
        let dir = tempfile::tempdir().unwrap();
        let map = dir.path().join(".plan/maps/node-handoff");
        std::fs::create_dir_all(map.join("tickets")).unwrap();
        std::fs::write(
            map.join("map.md"),
            "# Node handoff\n\n## Destination\nMove a branch.\n",
        )
        .unwrap();
        std::fs::write(
            map.join("tickets/01-transfer-scope.md"),
            ticket(1, "", "## Answer\nCommitted only."),
        )
        .unwrap();
        std::fs::write(map.join("tickets/02-implement.md"), ticket(2, "01", "")).unwrap();
        std::fs::write(map.join("tickets/02-duplicate.md"), ticket(2, "", "")).unwrap();
        std::fs::write(map.join("tickets/notes.md"), "# Notes\n").unwrap();
        std::fs::write(map.join("tickets/03-bad.md"), ticket(3, "soon", "")).unwrap();
        std::fs::create_dir_all(dir.path().join(".plan/maps/Bad Name")).unwrap();

        let loaded = load(dir.path(), "node-handoff").unwrap();
        assert_eq!(loaded.frontier, vec![2]);
        assert_eq!(
            loaded.nodes.len(),
            3,
            "the destination and two good tickets"
        );
        let ticket_two = loaded.nodes.iter().find(|n| n.number == 2).unwrap();
        assert_eq!(
            ticket_two.path.as_deref(),
            Some(".plan/maps/node-handoff/tickets/02-duplicate.md")
        );
        assert_eq!(loaded.diagnostics.len(), 3, "{:?}", loaded.diagnostics);
        let listed = list(dir.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            (
                listed[0].slug.as_str(),
                listed[0].tickets,
                listed[0].open_tickets
            ),
            ("node-handoff", 2, 1)
        );
        assert!(load(dir.path(), "../etc").is_err());
        assert!(load(dir.path(), "missing").is_err());
        assert!(list(tempfile::tempdir().unwrap().path())
            .unwrap()
            .is_empty());
    }
}
