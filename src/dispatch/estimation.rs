use crate::dispatch::issues::TicketMetadata;
use crate::ledger::LedgerEntry;
use crate::models::PlannerWorkPacket;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TicketComplexityEstimate {
    #[serde(default)]
    pub predicted_difficulty: Option<String>,
    #[serde(default)]
    pub predicted_cost_usd: Option<f64>,
    #[serde(default)]
    pub predicted_duration_seconds: Option<f64>,
}

struct EstimateInput<'a> {
    title: &'a str,
    summary: Option<&'a str>,
    objective: Option<&'a str>,
    task_class: Option<&'a str>,
    difficulty: Option<&'a str>,
    risk: Option<&'a str>,
    execution_disposition: Option<&'a str>,
    affected_files: &'a [String],
    acceptance_criteria: &'a [String],
    verification_commands: &'a [String],
    constraints: &'a [String],
}

pub fn estimate_ticket_complexity_from_metadata(
    ticket: &TicketMetadata,
) -> TicketComplexityEstimate {
    estimate_ticket_complexity(EstimateInput {
        title: ticket.title.as_deref().unwrap_or(""),
        summary: ticket.summary.as_deref(),
        objective: ticket.goal.as_deref().or(ticket.problem.as_deref()),
        task_class: ticket.task_class.as_deref(),
        difficulty: ticket.difficulty.as_deref(),
        risk: ticket.risk.as_deref(),
        execution_disposition: ticket.execution_disposition.as_deref(),
        affected_files: &ticket.affected_files,
        acceptance_criteria: &ticket.acceptance_criteria,
        verification_commands: &ticket.verification_commands,
        constraints: &ticket.constraints,
    })
}

pub fn estimate_ticket_complexity_from_plan(
    ticket: &PlannerWorkPacket,
) -> TicketComplexityEstimate {
    estimate_ticket_complexity(EstimateInput {
        title: &ticket.title,
        summary: Some(ticket.summary.as_str()),
        objective: Some(ticket.objective.as_str()),
        task_class: Some(ticket.task_class.as_str()),
        difficulty: Some(ticket.difficulty.as_str()),
        risk: Some(ticket.risk.as_str()),
        execution_disposition: Some(ticket.execution_disposition.as_str()),
        affected_files: &ticket.affected_files,
        acceptance_criteria: &ticket.acceptance_criteria,
        verification_commands: &ticket.verification_commands,
        constraints: &[],
    })
}

pub fn apply_estimate_to_ledger(ledger: &mut LedgerEntry, estimate: TicketComplexityEstimate) {
    ledger.predicted_difficulty = estimate.predicted_difficulty;
    ledger.predicted_cost_usd = estimate.predicted_cost_usd;
    ledger.predicted_duration_seconds = estimate.predicted_duration_seconds;
}

fn estimate_ticket_complexity(input: EstimateInput<'_>) -> TicketComplexityEstimate {
    let heuristic_score = heuristic_score(&input);
    let heuristic_rank = score_to_rank(heuristic_score);
    let explicit_rank = input
        .difficulty
        .and_then(canonical_difficulty)
        .map(rank_for_difficulty);
    let predicted_rank = explicit_rank.unwrap_or(heuristic_rank);
    let predicted_difficulty = input
        .difficulty
        .and_then(canonical_difficulty)
        .map(str::to_string)
        .or_else(|| Some(difficulty_label(predicted_rank).to_string()));
    let effective_rank = explicit_rank.map_or(predicted_rank, |rank| rank.max(predicted_rank));
    let predicted_duration_seconds = Some(duration_for_rank(effective_rank, heuristic_score));
    let predicted_cost_usd = Some(cost_for_rank(effective_rank, heuristic_score));

    TicketComplexityEstimate {
        predicted_difficulty,
        predicted_cost_usd,
        predicted_duration_seconds,
    }
}

fn heuristic_score(input: &EstimateInput<'_>) -> u32 {
    let mut score = 0u32;
    score += match input.task_class.map(normalize_label).as_deref() {
        Some("feature") | Some("refactor") => 2,
        Some("fix") | Some("test") => 1,
        Some("docs") | Some("chore") => 0,
        _ => 1,
    };
    score += match input.risk.map(normalize_label).as_deref() {
        Some("high") => 2,
        Some("medium") => 1,
        _ => 0,
    };
    score += count_score(input.affected_files.len(), 2, 5);
    score += count_score(input.acceptance_criteria.len(), 3, 6);
    score += count_score(input.verification_commands.len(), 2, 4);
    score += count_score(input.constraints.len(), 2, 4);

    let narrative = [
        input.title.to_string(),
        input.summary.unwrap_or("").to_string(),
        input.objective.unwrap_or("").to_string(),
        join_labels(input.constraints),
        join_labels(input.acceptance_criteria),
        join_labels(input.verification_commands),
    ]
    .join("\n")
    .to_lowercase();
    for keyword in [
        "migration",
        "integrat",
        "concurr",
        "parallel",
        "protocol",
        "security",
        "performance",
        "refactor",
        "state machine",
        "cross-cutting",
    ] {
        if narrative.contains(keyword) {
            score += 1;
        }
    }
    if input.execution_disposition.map(normalize_label).as_deref() == Some("human_required") {
        score += 1;
    }
    score
}

fn count_score(count: usize, medium_at: usize, hard_at: usize) -> u32 {
    match count {
        0..=1 => 0,
        n if n >= hard_at => 2,
        n if n >= medium_at => 1,
        _ => 0,
    }
}

fn normalize_label(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn join_labels(values: &[String]) -> String {
    values.join("\n")
}

fn canonical_difficulty(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "easy" => Some("easy"),
        "medium" => Some("medium"),
        "hard" => Some("hard"),
        _ => None,
    }
}

fn rank_for_difficulty(value: &str) -> u32 {
    match value {
        "easy" => 0,
        "medium" => 1,
        "hard" => 2,
        _ => 0,
    }
}

fn score_to_rank(score: u32) -> u32 {
    match score {
        0..=2 => 0,
        3..=5 => 1,
        _ => 2,
    }
}

fn difficulty_label(rank: u32) -> &'static str {
    match rank {
        0 => "easy",
        1 => "medium",
        _ => "hard",
    }
}

fn duration_for_rank(rank: u32, score: u32) -> f64 {
    let base = match rank {
        0 => 1_800.0,
        1 => 7_200.0,
        _ => 18_000.0,
    };
    base + (score as f64 * 600.0)
}

fn cost_for_rank(rank: u32, score: u32) -> f64 {
    let base = match rank {
        0 => 0.12,
        1 => 0.48,
        _ => 1.20,
    };
    base + (score as f64 * 0.08)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_ticket_metadata_with_explicit_difficulty() {
        let ticket = TicketMetadata {
            title: Some("Refactor the parser".to_string()),
            task_class: Some("refactor".to_string()),
            difficulty: Some("Hard".to_string()),
            risk: Some("high".to_string()),
            execution_disposition: Some("autonomous".to_string()),
            affected_files: vec!["src/parser.rs".to_string(), "src/token.rs".to_string()],
            acceptance_criteria: vec!["parser handles edge cases".to_string()],
            verification_commands: vec!["cargo test parser".to_string()],
            ..Default::default()
        };

        let estimate = estimate_ticket_complexity_from_metadata(&ticket);
        assert_eq!(estimate.predicted_difficulty.as_deref(), Some("hard"));
        assert!(estimate.predicted_cost_usd.unwrap() > 0.0);
        assert!(estimate.predicted_duration_seconds.unwrap() > 0.0);
    }

    #[test]
    fn estimates_plan_ticket_without_an_explicit_difficulty_fallback() {
        let ticket = PlannerWorkPacket {
            key: "child-1".to_string(),
            title: "Add integration coverage".to_string(),
            summary: "Exercise the full request path".to_string(),
            objective: "Add integration coverage".to_string(),
            task_class: "test".to_string(),
            difficulty: "medium".to_string(),
            risk: "medium".to_string(),
            execution_disposition: "autonomous".to_string(),
            recommended_routing: crate::models::RecommendedRouting {
                capability: "review".to_string(),
                min_tier: "standard".to_string(),
            },
            affected_areas: vec!["routing".to_string()],
            affected_files: vec!["src/routing/mod.rs".to_string()],
            acceptance_criteria: vec!["integration coverage lands".to_string()],
            verification_commands: vec!["cargo test".to_string()],
            depends_on: vec![],
            duplicate_evidence: vec![],
            uncovered_reason: "no existing plan covers it".to_string(),
        };

        let estimate = estimate_ticket_complexity_from_plan(&ticket);
        assert_eq!(estimate.predicted_difficulty.as_deref(), Some("medium"));
        assert!(estimate.predicted_cost_usd.unwrap() > 0.0);
        assert!(estimate.predicted_duration_seconds.unwrap() > 0.0);
    }
}
