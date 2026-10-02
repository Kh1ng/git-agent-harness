//! An epic as a graph: the epic, every issue under it, and the issues they
//! wait on. Pure: `fetch` gathers the facts, this decides the shape.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// What the provider says about one issue. Body lines are the convention
/// GAH's PM publish writes (`Parent: #12`, `Blocked by: #3, #4`); native
/// relations come from GitHub sub-issues and dependencies or GitLab links.
#[derive(Debug, Clone, Default)]
pub struct IssueFacts {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub open: bool,
    pub labels: Vec<String>,
    pub body: String,
    pub native_children: Vec<u64>,
    pub native_blockers: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    /// Closed.
    Done,
    /// Open, nothing open blocks it, and no open children: workable now.
    Ready,
    /// Open and waiting on at least one open issue.
    Blocked,
    /// Open, unblocked, and its open children carry the work.
    Parent,
    /// A map-file ticket answered as out of scope. Unlike `Done`, it does
    /// not unblock the tickets that wait on it.
    RuledOut,
    /// A map-file ticket someone has claimed: open, but not free to take.
    Claimed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub labels: Vec<String>,
    pub state: NodeState,
    /// Distance from the epic along child links; blockers outside the epic
    /// have none.
    pub depth: Option<usize>,
    /// Open or unknown issues this one waits on.
    pub waiting_on: Vec<u64>,
    /// For a map-file ticket, its path in the repository; `url` is empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// `from` is the parent of `to`.
    Child,
    /// `from` must close before `to` can start.
    Blocks,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Edge {
    pub from: u64,
    pub to: u64,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanMap {
    pub epic: u64,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Ready issues, the work that can start now.
    pub frontier: Vec<u64>,
    /// Referenced issues the listing did not include (deleted, or in another
    /// project).
    pub missing: Vec<u64>,
    /// The `.plan/maps/` slug when the map came from files, not issues.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Problems reading a map file that did not stop the map.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EpicSummary {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub open: bool,
    pub children: usize,
    pub open_children: usize,
}

/// Children deeper than this are left out, so a parent cycle cannot run away.
const MAX_DEPTH: usize = 8;

/// `#N` references on the first body line that starts with `prefix`
/// (case-insensitive). Other forms (`owner/repo#N`, URLs, prose) are skipped:
/// a map shows one repository.
pub fn line_references(body: &str, prefix: &str) -> Vec<u64> {
    let Some(line) = body.lines().map(str::trim).find(|line| {
        line.len() >= prefix.len()
            && line.is_char_boundary(prefix.len())
            && line[..prefix.len()].eq_ignore_ascii_case(prefix)
    }) else {
        return Vec::new();
    };
    let rest = &line[prefix.len()..];
    let mut numbers = Vec::new();
    for (index, _) in rest.match_indices('#') {
        let preceded_by_name = rest[..index]
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '/' || c == '-' || c == '_');
        if preceded_by_name {
            continue;
        }
        let digits: String = rest[index + 1..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(number) = digits.parse() {
            numbers.push(number);
        }
    }
    numbers
}

fn body_parent(issue: &IssueFacts) -> Option<u64> {
    line_references(&issue.body, "Parent:").first().copied()
}

/// Blockers from the body, and whether any reference could not be resolved to
/// an issue here. Dispatch's parser decides what a `Blocked by:` line means,
/// so a line it rejects (duplicates, bad syntax) or a reference to another
/// project keeps the issue waiting instead of reading as ready.
fn body_blockers(issue: &IssueFacts) -> (Vec<u64>, bool) {
    let mut blockers = line_references(&issue.body, "Depends on:");
    let mut unresolved = false;
    match crate::dispatch::dependencies::parse_dependency_line(&issue.body) {
        Ok(references) => {
            for reference in references.into_iter().flatten() {
                match reference.parse() {
                    Ok(number) => blockers.push(number),
                    Err(_) => unresolved = true,
                }
            }
        }
        Err(_) => unresolved = true,
    }
    (blockers, unresolved)
}

/// Parent to children, from native links and `Parent:` lines together.
fn children_by_parent(issues: &BTreeMap<u64, IssueFacts>) -> BTreeMap<u64, BTreeSet<u64>> {
    let mut children: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
    for issue in issues.values() {
        for child in &issue.native_children {
            children.entry(issue.number).or_default().insert(*child);
        }
        if let Some(parent) = body_parent(issue) {
            if parent != issue.number {
                children.entry(parent).or_default().insert(issue.number);
            }
        }
    }
    children
}

/// Issues that have children or carry an `epic` or `wayfinder:map` label,
/// open ones first, then newest. `native` holds GitHub's own (total, open)
/// sub-issue counts; a child linked both ways is counted once, so the larger
/// of the two counts is shown.
pub fn epics(
    issues: &BTreeMap<u64, IssueFacts>,
    native: &BTreeMap<u64, (usize, usize)>,
) -> Vec<EpicSummary> {
    let children = children_by_parent(issues);
    let mut epics: Vec<EpicSummary> = issues
        .values()
        .filter(|issue| {
            children.contains_key(&issue.number)
                || native.contains_key(&issue.number)
                || issue
                    .labels
                    .iter()
                    .any(|label| label == "epic" || label == "wayfinder:map")
        })
        .map(|issue| {
            let kids = children.get(&issue.number);
            let (native_total, native_open) =
                native.get(&issue.number).copied().unwrap_or_default();
            let open_kids = kids.map_or(0, |kids| {
                kids.iter()
                    .filter(|kid| issues.get(kid).is_some_and(|kid| kid.open))
                    .count()
            });
            EpicSummary {
                number: issue.number,
                title: issue.title.clone(),
                url: issue.url.clone(),
                open: issue.open,
                children: kids.map_or(0, BTreeSet::len).max(native_total),
                open_children: open_kids.max(native_open),
            }
        })
        .collect();
    epics.sort_by(|a, b| b.open.cmp(&a.open).then(b.number.cmp(&a.number)));
    epics
}

/// The epic's descendants, nearest first, with their depth.
pub fn descendants(epic: u64, issues: &BTreeMap<u64, IssueFacts>) -> BTreeMap<u64, usize> {
    let children = children_by_parent(issues);
    let mut depth = BTreeMap::from([(epic, 0)]);
    let mut queue = VecDeque::from([epic]);
    while let Some(parent) = queue.pop_front() {
        let level = depth[&parent];
        if level >= MAX_DEPTH {
            continue;
        }
        for child in children.get(&parent).into_iter().flatten() {
            if !depth.contains_key(child) {
                depth.insert(*child, level + 1);
                queue.push_back(*child);
            }
        }
    }
    depth
}

pub fn build(epic: u64, issues: &BTreeMap<u64, IssueFacts>) -> PlanMap {
    let depth = descendants(epic, issues);
    let mut edges = BTreeSet::new();
    let children = children_by_parent(issues);
    for (parent, kids) in &children {
        if depth.contains_key(parent) {
            for kid in kids.iter().filter(|kid| depth.contains_key(kid)) {
                edges.insert(Edge {
                    from: *parent,
                    to: *kid,
                    kind: EdgeKind::Child,
                });
            }
        }
    }
    let mut blockers: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
    let mut unresolved_issues = BTreeSet::new();
    for number in depth.keys() {
        let Some(issue) = issues.get(number) else {
            continue;
        };
        let (from_body, unresolved) = body_blockers(issue);
        if unresolved {
            unresolved_issues.insert(*number);
        }
        let found = issue.native_blockers.iter().copied().chain(from_body);
        for blocker in found.filter(|blocker| blocker != number) {
            blockers.entry(*number).or_default().insert(blocker);
            edges.insert(Edge {
                from: blocker,
                to: *number,
                kind: EdgeKind::Blocks,
            });
        }
    }

    let mut members: BTreeSet<u64> = depth.keys().copied().collect();
    members.extend(blockers.values().flatten());
    let open = |number: &u64| issues.get(number).is_some_and(|issue| issue.open);
    let mut nodes = Vec::new();
    let mut missing = Vec::new();
    for number in &members {
        let Some(issue) = issues.get(number) else {
            missing.push(*number);
            continue;
        };
        let waiting_on: Vec<u64> = blockers
            .get(number)
            .into_iter()
            .flatten()
            .copied()
            // A blocker the listing lacks is unknown, not closed.
            .filter(|blocker| open(blocker) || !issues.contains_key(blocker))
            .collect();
        let has_open_children = depth.contains_key(number)
            && children
                .get(number)
                .is_some_and(|kids| kids.iter().any(|kid| depth.contains_key(kid) && open(kid)));
        let state = if !issue.open {
            NodeState::Done
        } else if !waiting_on.is_empty() || unresolved_issues.contains(number) {
            NodeState::Blocked
        } else if has_open_children {
            NodeState::Parent
        } else {
            NodeState::Ready
        };
        nodes.push(Node {
            number: *number,
            title: issue.title.clone(),
            url: issue.url.clone(),
            labels: issue.labels.clone(),
            state,
            depth: depth.get(number).copied(),
            waiting_on,
            path: None,
        });
    }
    let frontier = nodes
        .iter()
        .filter(|node| {
            node.state == NodeState::Ready && node.number != epic && node.depth.is_some()
        })
        .map(|node| node.number)
        .collect();
    PlanMap {
        epic,
        nodes,
        edges: edges.into_iter().collect(),
        frontier,
        missing,
        file: None,
        diagnostics: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(number: u64, open: bool, body: &str) -> IssueFacts {
        IssueFacts {
            number,
            title: format!("Issue {number}"),
            url: format!("https://example.test/issues/{number}"),
            open,
            body: body.into(),
            ..IssueFacts::default()
        }
    }

    fn index(issues: Vec<IssueFacts>) -> BTreeMap<u64, IssueFacts> {
        issues
            .into_iter()
            .map(|issue| (issue.number, issue))
            .collect()
    }

    fn state(map: &PlanMap, number: u64) -> NodeState {
        map.nodes
            .iter()
            .find(|node| node.number == number)
            .unwrap()
            .state
    }

    #[test]
    fn references_come_from_the_named_line_and_skip_other_repositories() {
        let body =
            "Context\nParent: #935 (post-MVP), see owner/repo#7\nBlocked by: #12, #34 and #56.";
        assert_eq!(line_references(body, "Parent:"), vec![935]);
        assert_eq!(line_references(body, "blocked by:"), vec![12, 34, 56]);
        assert_eq!(line_references(body, "Depends on:"), Vec::<u64>::new());
        assert_eq!(
            line_references("Blocked by: control plane 4/7", "Blocked by:"),
            Vec::<u64>::new()
        );
    }

    #[test]
    fn the_frontier_is_open_unblocked_leaf_work_under_the_epic() {
        let issues = index(vec![
            issue(1, true, "the epic"),
            issue(2, false, "Parent: #1"),
            issue(3, true, "Parent: #1\nBlocked by: #2"),
            issue(4, true, "Parent: #1\nBlocked by: #3"),
            issue(5, true, "Parent: #1"),
            issue(6, true, "Parent: #5"),
            issue(7, true, "unrelated"),
        ]);
        let map = build(1, &issues);
        assert_eq!(state(&map, 2), NodeState::Done);
        assert_eq!(
            state(&map, 3),
            NodeState::Ready,
            "its only blocker is closed"
        );
        assert_eq!(state(&map, 4), NodeState::Blocked);
        assert_eq!(
            state(&map, 5),
            NodeState::Parent,
            "its open child carries the work"
        );
        assert_eq!(state(&map, 1), NodeState::Parent);
        assert_eq!(map.frontier, vec![3, 6]);
        assert!(map.nodes.iter().all(|node| node.number != 7));
        assert!(map.edges.contains(&Edge {
            from: 3,
            to: 4,
            kind: EdgeKind::Blocks
        }));
        assert!(map.edges.contains(&Edge {
            from: 5,
            to: 6,
            kind: EdgeKind::Child
        }));
    }

    #[test]
    fn native_links_count_and_outside_blockers_appear_without_depth() {
        let mut epic = issue(10, true, "");
        epic.native_children = vec![11];
        let mut child = issue(11, true, "");
        child.native_blockers = vec![20];
        let issues = index(vec![
            epic,
            child,
            issue(20, true, "elsewhere"),
            issue(12, true, "Blocked by: #99\nParent: #10"),
        ]);
        let map = build(10, &issues);
        let outside = map.nodes.iter().find(|node| node.number == 20).unwrap();
        assert_eq!(outside.depth, None);
        assert_eq!(state(&map, 11), NodeState::Blocked);
        assert_eq!(map.missing, vec![99]);
        assert_eq!(
            state(&map, 12),
            NodeState::Blocked,
            "a blocker the listing lacks is unknown, not closed"
        );
        assert!(!map.frontier.contains(&12));
        assert!(
            map.frontier.iter().all(|number| *number != 20),
            "blockers outside the epic are not its frontier"
        );
    }

    #[test]
    fn bodies_dispatch_rejects_never_read_as_ready() {
        let issues = index(vec![
            issue(1, true, ""),
            issue(2, true, "Parent: #1\nBlocked by: #9, #9"),
            issue(3, true, "Parent: #1\nBlocked by: #9\nBlocked by: #8"),
            issue(4, true, "Parent: #1\nBlocked by #9"),
            issue(5, true, "Parent: #1\nBlocked by: github:o/r#7"),
            issue(6, true, "Parent: #1\nBlocked by: #7"),
            issue(7, false, "Parent: #1"),
        ]);
        let map = build(1, &issues);
        for number in [2, 3, 4, 5] {
            assert_eq!(state(&map, number), NodeState::Blocked, "#{number}");
        }
        assert_eq!(map.frontier, vec![6]);
    }

    #[test]
    fn a_parent_cycle_terminates() {
        let issues = index(vec![
            issue(1, true, "Parent: #2"),
            issue(2, true, "Parent: #1"),
        ]);
        let map = build(1, &issues);
        assert_eq!(map.nodes.len(), 2);
    }

    #[test]
    fn epics_are_parents_or_labelled_open_first() {
        let mut labelled = issue(30, false, "");
        labelled.labels = vec!["epic".into()];
        let issues = index(vec![
            issue(1, true, ""),
            issue(2, false, "Parent: #1"),
            labelled,
            issue(40, true, ""),
        ]);
        let found: Vec<(u64, usize, usize)> = epics(&issues, &BTreeMap::from([(40, (3, 2))]))
            .iter()
            .map(|epic| (epic.number, epic.children, epic.open_children))
            .collect();
        assert_eq!(found, vec![(40, 3, 2), (1, 1, 0), (30, 0, 0)]);
    }
}
