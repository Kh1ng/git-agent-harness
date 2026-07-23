#[derive(Copy, Clone)]
enum PlainSourceSection {
    Problem,
    AcceptanceCriteria,
    VerificationCommands,
    NonGoals,
}

#[derive(Default)]
pub(super) struct UnheadedSourceSections {
    pub(super) problem: Option<String>,
    pub(super) acceptance_criteria: Vec<String>,
    pub(super) verification_commands: Vec<String>,
    pub(super) non_goals: Option<String>,
}

fn normalize_plain_section_heading(raw_heading: &str) -> Option<String> {
    let trimmed = raw_heading.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with('-')
        || trimmed.starts_with('*')
        || trimmed.starts_with('>')
    {
        return None;
    }

    let normalized: String = trimmed
        .trim_end_matches(':')
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c.is_ascii_whitespace() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();
    let normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    (!normalized.is_empty()).then_some(normalized)
}

fn classify_unheaded_source_section(raw_heading: &str) -> Option<PlainSourceSection> {
    match normalize_plain_section_heading(raw_heading).as_deref() {
        Some("exact expected behavior") | Some("expected behavior") | Some("expected") => {
            Some(PlainSourceSection::Problem)
        }
        Some("acceptance criteria") => Some(PlainSourceSection::AcceptanceCriteria),
        Some("test or validation command")
        | Some("test and validation command")
        | Some("test command")
        | Some("verification command")
        | Some("verification commands") => Some(PlainSourceSection::VerificationCommands),
        Some("non goals") => Some(PlainSourceSection::NonGoals),
        _ => None,
    }
}

pub(super) fn extract(body: &str) -> UnheadedSourceSections {
    let mut active_section = None;
    let mut problem = String::new();
    let mut acceptance_criteria = String::new();
    let mut verification_commands = String::new();
    let mut non_goals = String::new();

    for line in body.lines() {
        if let Some(section) = classify_unheaded_source_section(line) {
            active_section = Some(section);
            continue;
        }
        if line.trim_start().starts_with('#') {
            active_section = None;
            continue;
        }
        if active_section.is_some() && is_known_plain_metadata_boundary(line) {
            active_section = None;
            continue;
        }

        if let Some(section) = active_section {
            let destination = match section {
                PlainSourceSection::Problem => &mut problem,
                PlainSourceSection::AcceptanceCriteria => &mut acceptance_criteria,
                PlainSourceSection::VerificationCommands => &mut verification_commands,
                PlainSourceSection::NonGoals => &mut non_goals,
            };
            destination.push_str(line);
            destination.push('\n');
        }
    }

    UnheadedSourceSections {
        problem: nonempty(problem),
        acceptance_criteria: section_entries_from_text(&acceptance_criteria),
        verification_commands: section_entries_from_text(&verification_commands),
        non_goals: nonempty(non_goals),
    }
}

fn is_known_plain_metadata_boundary(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed != line || trimmed.starts_with('-') || trimmed.starts_with('*') {
        return false;
    }
    let Some((label, _)) = trimmed.split_once(':') else {
        return false;
    };
    let normalized = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c.is_ascii_whitespace() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    matches!(
        normalized.as_str(),
        "risk"
            | "priority"
            | "mode"
            | "backend"
            | "difficulty"
            | "task class"
            | "context"
            | "task"
            | "validation"
            | "files"
            | "files involved"
            | "files likely touched"
            | "function module"
            | "source"
            | "goal"
            | "recommended backend"
            | "recommended model"
            | "suggested mr title"
            | "local coder suitability"
            | "parent issue"
    ) || normalized.starts_with("ticket road map")
}

fn nonempty(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

pub(super) fn section_entries_from_text(text: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut current = String::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(item) = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .or_else(|| line.strip_prefix('-'))
            .or_else(|| line.strip_prefix('*'))
        {
            if !current.is_empty() {
                entries.push(std::mem::take(&mut current));
            }
            current.push_str(item.trim());
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
    }
    if !current.is_empty() {
        entries.push(current);
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::super::source_issue_context::{
        render_source_issue_contract, verified_post_budget_source_contract,
    };
    use super::extract;
    use crate::context::{self, ContextConfig};
    use crate::dispatch::issues::IssueDetails;

    #[test]
    fn extracts_sportsball_style_plain_sections() {
        let sections = extract(include_str!(
            "../../../../tests/fixtures/sportsball_issue_40.md"
        ));

        assert!(sections
            .problem
            .as_deref()
            .unwrap()
            .contains("capture_git_state uses subprocess"));
        assert_eq!(sections.acceptance_criteria.len(), 2);
        assert_eq!(sections.verification_commands.len(), 2);
        assert!(sections
            .non_goals
            .as_deref()
            .unwrap()
            .contains("Does not write to DB itself"));
        assert!(!sections.non_goals.as_deref().unwrap().contains("Risk: low"));
    }

    #[test]
    fn markdown_heading_ends_a_plain_section() {
        let sections = extract(
            "Expected behavior\nKeep this requirement.\n\n## Implementation status\nDo not treat this as a requirement.",
        );

        assert_eq!(sections.problem.as_deref(), Some("Keep this requirement."));
    }

    #[test]
    fn metadata_label_ends_non_goals_before_status_and_roadmap_text() {
        let sections = extract(
            "Non-goals:\n- Does not write to DB itself.\n\nRisk: low\n\nTicket road map:\n1) unrelated",
        );

        assert_eq!(
            sections.non_goals.as_deref(),
            Some("- Does not write to DB itself.")
        );
    }

    #[test]
    fn ordinary_colon_prose_does_not_truncate_plain_requirements() {
        let sections = extract(
            "Expected behavior\nKeep the first requirement.\nNote: this context remains part of the requirement.\nReproduce here: https://example.test/case\nKeep the final requirement.",
        );

        let problem = sections.problem.as_deref().unwrap();
        assert!(problem.contains("Note: this context remains part of the requirement."));
        assert!(problem.contains("Reproduce here: https://example.test/case"));
        assert!(problem.contains("Keep the final requirement."));
    }

    #[test]
    fn wrapped_list_items_preserve_continuation_lines() {
        let sections = extract(
            "Acceptance criteria\n- First criterion continues here\n  and wraps to a second physical line with more detail.\n- Second criterion.\n",
        );

        assert_eq!(
            sections.acceptance_criteria,
            vec![
                "First criterion continues here\nand wraps to a second physical line with more detail.",
                "Second criterion."
            ]
        );
    }

    #[test]
    fn unbulleted_multiline_entries_remain_one_bounded_entry() {
        let sections = extract(
            "Test or validation command\npython -m pytest tests/test_one.py\nwith the integration environment enabled\n",
        );

        assert_eq!(
            sections.verification_commands,
            vec!["python -m pytest tests/test_one.py\nwith the integration environment enabled"]
        );
    }

    #[test]
    fn source_contract_retains_plain_sections_alongside_a_goal() {
        let issue = IssueDetails {
            number: "40".into(),
            title: "Sportsball: unheaded source sections".into(),
            body: include_str!("../../../../tests/fixtures/sportsball_issue_40.md").into(),
            labels: vec!["bug".into()],
            state: None,
        };

        let contract = render_source_issue_contract(&issue);
        assert!(contract.contains("### Problem"));
        assert!(contract.contains("### Expected Behavior"));
        assert!(contract.contains("### Acceptance Criteria"));
        assert!(contract.contains("### Verification Commands"));
        assert!(contract.contains("### Non-goals"));
        assert!(contract.contains("capture_git_state uses subprocess"));
        assert!(contract.contains("In a real repo, git state fields are populated."));
        assert!(!contract.contains("Ticket road map for parent issue"));
        assert!(!contract.contains("## Exact expected behavior"));

        let prompt = format!(
            "## Review Pack\n\n{}\n\n## Prior Run State\n\nn/a\n\n## Diff\n\n{}\n",
            contract,
            "x".repeat(4_000)
        );
        let built = context::enforce(
            &prompt,
            &ContextConfig {
                soft_limit_tokens: 10,
                hard_limit_tokens: 2_000,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(built.prompt.contains(&contract));
        assert_eq!(
            verified_post_budget_source_contract(Some(&contract), &built.prompt).unwrap(),
            Some(contract.as_str())
        );
    }

    #[test]
    fn provider_labels_do_not_block_unstructured_body_fallback() {
        let issue = IssueDetails {
            number: "41".into(),
            title: "No headings".into(),
            body: "This issue body is prose only, so fallback is required.\n\nNo markdown headings and no known sections are present."
                .into(),
            labels: vec!["bug".into(), "area:control-plane".into()],
            state: None,
        };

        let contract = render_source_issue_contract(&issue);
        assert!(contract.contains("### Issue Description"));
        assert!(contract.contains("### Constraints"));
        assert!(contract.contains("-   bug"));
        assert!(contract.contains("This issue body is prose only, so fallback is required."));
    }

    #[test]
    fn additional_markdown_details_do_not_duplicate_the_raw_issue_body() {
        let issue = IssueDetails {
            number: "42".into(),
            title: "Examples-only contract".into(),
            body: "## Examples\n\nSome content here that should not be duplicated.\n".into(),
            labels: vec!["bug".into()],
            state: None,
        };

        let contract = render_source_issue_contract(&issue);
        assert!(contract.contains("### Examples"));
        assert!(!contract.contains("### Issue Description"));
        assert_eq!(
            contract
                .matches("Some content here that should not be duplicated.")
                .count(),
            1
        );
    }
}
