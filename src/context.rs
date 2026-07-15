//! Deterministic context budgeting for coding and review prompts.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_true() -> bool {
    true
}

fn default_soft() -> u64 {
    80_000
}

fn default_hard() -> u64 {
    150_000
}

fn default_recent() -> u64 {
    20_000
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ContextConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_soft")]
    pub soft_limit_tokens: u64,
    #[serde(default = "default_hard")]
    pub hard_limit_tokens: u64,
    #[serde(default)]
    pub compact_after_tool_calls: u64,
    #[serde(default = "default_true")]
    pub fresh_context_on_review: bool,
    #[serde(default = "default_true")]
    pub fresh_context_on_fix: bool,
    #[serde(default)]
    pub include_full_git_history: bool,
    #[serde(default)]
    pub include_full_worker_transcript_in_review: bool,
    #[serde(default = "default_recent")]
    pub recent_history_tokens: u64,
    #[serde(default)]
    pub profiles: HashMap<String, ContextOverride>,
    #[serde(default)]
    pub backends: HashMap<String, ContextOverride>,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            soft_limit_tokens: default_soft(),
            hard_limit_tokens: default_hard(),
            compact_after_tool_calls: 20,
            fresh_context_on_review: true,
            fresh_context_on_fix: true,
            include_full_git_history: false,
            include_full_worker_transcript_in_review: false,
            recent_history_tokens: default_recent(),
            profiles: HashMap::new(),
            backends: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub struct ContextOverride {
    pub enabled: Option<bool>,
    pub soft_limit_tokens: Option<u64>,
    pub hard_limit_tokens: Option<u64>,
    pub compact_after_tool_calls: Option<u64>,
    pub fresh_context_on_review: Option<bool>,
    pub fresh_context_on_fix: Option<bool>,
    pub include_full_git_history: Option<bool>,
    pub include_full_worker_transcript_in_review: Option<bool>,
    pub recent_history_tokens: Option<u64>,
}

impl ContextConfig {
    pub fn effective(&self, profile: &str, backend: &str) -> Self {
        let mut out = self.clone();
        for override_cfg in [self.profiles.get(profile), self.backends.get(backend)]
            .into_iter()
            .flatten()
        {
            if let Some(v) = override_cfg.enabled {
                out.enabled = v;
            }
            if let Some(v) = override_cfg.soft_limit_tokens {
                out.soft_limit_tokens = v;
            }
            if let Some(v) = override_cfg.hard_limit_tokens {
                out.hard_limit_tokens = v;
            }
            if let Some(v) = override_cfg.compact_after_tool_calls {
                out.compact_after_tool_calls = v;
            }
            if let Some(v) = override_cfg.fresh_context_on_review {
                out.fresh_context_on_review = v;
            }
            if let Some(v) = override_cfg.fresh_context_on_fix {
                out.fresh_context_on_fix = v;
            }
            if let Some(v) = override_cfg.include_full_git_history {
                out.include_full_git_history = v;
            }
            if let Some(v) = override_cfg.include_full_worker_transcript_in_review {
                out.include_full_worker_transcript_in_review = v;
            }
            if let Some(v) = override_cfg.recent_history_tokens {
                out.recent_history_tokens = v;
            }
        }
        out.profiles.clear();
        out.backends.clear();
        out
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ContextBuild {
    pub prompt: String,
    pub estimated_tokens_before_reduction: u64,
    pub estimated_tokens_after_reduction: u64,
    pub compacted: bool,
    pub largest_sections: Vec<ContextSectionSize>,
    /// Every named prompt section in the prompt actually supplied to the
    /// backend after compaction. This makes the context artifact an audit
    /// record rather than just a token counter.
    pub sources: Vec<ContextSource>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ContextSectionSize {
    pub name: String,
    pub estimated_tokens: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ContextSource {
    pub name: String,
    pub bytes: u64,
    pub estimated_tokens: u64,
}

/// Conservative provider-independent estimate: four UTF-8 bytes per token.
pub fn estimate_tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(4)
}

pub fn enforce(prompt: &str, cfg: &ContextConfig) -> Result<ContextBuild> {
    let before = estimate_tokens(prompt);
    if !cfg.enabled || (before <= cfg.soft_limit_tokens && before <= cfg.hard_limit_tokens) {
        return Ok(ContextBuild {
            prompt: prompt.to_string(),
            estimated_tokens_before_reduction: before,
            estimated_tokens_after_reduction: before,
            compacted: false,
            largest_sections: section_sizes(prompt),
            sources: context_sources(prompt),
        });
    }

    let mut sections = split_sections(prompt);
    // Reduction order deliberately removes only non-critical sections first.
    for name in [
        "Manager Memory",
        "Git History",
        "Repository Map",
        "Context",
        "History",
    ] {
        for section in &mut sections {
            if section.name.contains(name) && !section.protected {
                section.body = format!(
                    "(compacted; retrieve relevant {} on demand)\n",
                    section.name
                );
            }
        }
    }
    let mut after = render_sections(&sections);
    if estimate_tokens(&after) > cfg.soft_limit_tokens {
        let removable: Vec<usize> = sections
            .iter()
            .enumerate()
            .filter_map(|(i, section)| (!section.protected).then_some(i))
            .collect();
        for index in removable {
            sections[index].body = "(compacted; retrieve on demand)\n".to_string();
            after = render_sections(&sections);
            if estimate_tokens(&after) <= cfg.soft_limit_tokens {
                break;
            }
        }
    }

    let after_tokens = estimate_tokens(&after);
    if after_tokens > cfg.hard_limit_tokens {
        bail!(
            "context_limit_exceeded: estimated {} tokens remains above hard limit {}",
            after_tokens,
            cfg.hard_limit_tokens
        );
    }
    let largest_sections = section_sizes(&after);
    let sources = context_sources(&after);
    Ok(ContextBuild {
        prompt: after,
        estimated_tokens_before_reduction: before,
        estimated_tokens_after_reduction: after_tokens,
        compacted: true,
        // These are an audit of the prompt that is actually sent, not the
        // pre-compaction prompt that was merely considered.
        largest_sections,
        sources,
    })
}

#[derive(Debug)]
struct Section {
    name: String,
    body: String,
    protected: bool,
}

fn split_sections(prompt: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut current = Section {
        name: "Preamble".into(),
        body: String::new(),
        protected: true,
    };
    for line in prompt.lines() {
        if let Some(name) = line.strip_prefix("## ") {
            sections.push(current);
            let protected = [
                "Focus",
                "Live Task Pack",
                "Safety",
                "Acceptance Criteria",
                "Verification Commands",
                "Repair Findings",
                "Source Issue Contract",
                "Source Issue Lookup",
                "Warning",
                "Current Git",
                "Unresolved",
            ]
            .iter()
            .any(|marker| name.contains(marker));
            current = Section {
                name: name.trim().to_string(),
                body: String::new(),
                protected,
            };
        } else {
            current.body.push_str(line);
            current.body.push('\n');
        }
    }
    sections.push(current);
    sections
}

fn render_sections(sections: &[Section]) -> String {
    let mut out = String::new();
    for section in sections {
        if section.name == "Preamble" {
            out.push_str(&section.body);
        } else {
            out.push_str("## ");
            out.push_str(&section.name);
            out.push_str("\n\n");
            out.push_str(&section.body);
        }
    }
    out
}

fn section_sizes(prompt: &str) -> Vec<ContextSectionSize> {
    let mut sizes: Vec<_> = split_sections(prompt)
        .into_iter()
        .map(|section| ContextSectionSize {
            name: section.name,
            estimated_tokens: estimate_tokens(&section.body),
        })
        .collect();
    sizes.sort_by_key(|size| std::cmp::Reverse(size.estimated_tokens));
    sizes.truncate(5);
    sizes
}

fn context_sources(prompt: &str) -> Vec<ContextSource> {
    split_sections(prompt)
        .into_iter()
        .filter(|section| section.name != "Preamble")
        .map(|section| ContextSource {
            name: section.name,
            bytes: section.body.len() as u64,
            estimated_tokens: estimate_tokens(&section.body),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_focus_while_compacting_memory() {
        let cfg = ContextConfig {
            soft_limit_tokens: 20,
            hard_limit_tokens: 100,
            ..Default::default()
        };
        let prompt = "## Manager Memory\nold output old output old output old output\n## Focus\nACCEPTANCE MUST REMAIN\n";
        let result = enforce(prompt, &cfg).unwrap();
        assert!(result.compacted);
        assert!(result.prompt.contains("ACCEPTANCE MUST REMAIN"));
        assert!(!result.prompt.contains("old output old output old output"));
    }

    #[test]
    fn refuses_when_protected_context_alone_exceeds_hard_limit() {
        let cfg = ContextConfig {
            soft_limit_tokens: 1,
            hard_limit_tokens: 2,
            ..Default::default()
        };
        let err = enforce(
            "## Focus\nthis protected acceptance text cannot be removed",
            &cfg,
        )
        .unwrap_err();
        assert!(err.to_string().contains("context_limit_exceeded"));
    }

    #[test]
    fn records_named_context_sources_for_audit() {
        let result = enforce(
            "Preamble\n## Project Brief\nstable facts\n## Live Task Pack\nTICKET-1\n## Focus\nfix it\n",
            &ContextConfig::default(),
        )
        .unwrap();

        assert_eq!(
            result
                .sources
                .iter()
                .map(|source| source.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Project Brief", "Live Task Pack", "Focus"]
        );
        assert!(result.sources.iter().all(|source| source.bytes > 0));
    }

    #[test]
    fn preserves_live_task_pack_and_reports_post_compaction_sources() {
        let cfg = ContextConfig {
            soft_limit_tokens: 45,
            hard_limit_tokens: 100,
            ..Default::default()
        };
        let prompt = "## Project Brief\nvery old context very old context very old context very old context very old context very old context\n## Live Task Pack\nAcceptance: preserve this requirement\nVerification: cargo test\n## Focus\nSee the Live Task Pack above.\n";

        let result = enforce(prompt, &cfg).unwrap();

        assert!(result.compacted);
        assert!(result
            .prompt
            .contains("Acceptance: preserve this requirement"));
        assert!(result.prompt.contains("Verification: cargo test"));
        let project_brief = result
            .sources
            .iter()
            .find(|source| source.name == "Project Brief")
            .unwrap();
        assert!(project_brief.estimated_tokens < 10);
        assert!(result
            .sources
            .iter()
            .any(|source| source.name == "Live Task Pack"));
    }

    #[test]
    fn preserves_repair_findings_while_compacting_other_context() {
        let cfg = ContextConfig {
            soft_limit_tokens: 30,
            hard_limit_tokens: 100,
            ..Default::default()
        };
        let prompt = "## Project Brief\nold old old old old old old old old old old old\n## Repair Findings\nBlocking findings:\n- src/lib.rs: retry loses state\n## Focus\nRepair the reviewed branch.\n";

        let result = enforce(prompt, &cfg).unwrap();

        assert!(result.compacted);
        assert!(result.prompt.contains("src/lib.rs: retry loses state"));
        assert!(result
            .sources
            .iter()
            .any(|source| source.name == "Repair Findings"));
    }

    #[test]
    fn refuses_to_silently_compact_oversized_live_task_pack() {
        let cfg = ContextConfig {
            soft_limit_tokens: 10,
            hard_limit_tokens: 20,
            ..Default::default()
        };
        let err = enforce(
            &format!(
                "## Live Task Pack\n{}\n## Focus\nSee pack\n",
                "x".repeat(200)
            ),
            &cfg,
        )
        .unwrap_err();

        assert!(err.to_string().contains("context_limit_exceeded"));
    }

    #[test]
    fn profile_and_backend_overrides_are_applied() {
        let mut cfg = ContextConfig::default();
        cfg.profiles.insert(
            "repo".into(),
            ContextOverride {
                soft_limit_tokens: Some(10),
                ..Default::default()
            },
        );
        cfg.backends.insert(
            "claude".into(),
            ContextOverride {
                hard_limit_tokens: Some(20),
                ..Default::default()
            },
        );
        let effective = cfg.effective("repo", "claude");
        assert_eq!(effective.soft_limit_tokens, 10);
        assert_eq!(effective.hard_limit_tokens, 20);
    }
}
