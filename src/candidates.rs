use crate::models::{
    Candidate, CandidateArtifact, CandidateCounts, GateArtifact, GateFinding, ScoutArtifact,
    ScoutFinding,
};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn run(gate_artifact: &str, include_warnings: bool, out_root: &str) -> Result<()> {
    let gate_path = Path::new(gate_artifact).join("gate.json");
    let gate: GateArtifact = serde_json::from_str(&fs::read_to_string(&gate_path)?)?;

    let scout = if let Some(dir) = gate.source_scout_artifact.as_ref() {
        let scout_path = Path::new(dir).join("scout.json");
        Some(serde_json::from_str::<ScoutArtifact>(&fs::read_to_string(
            scout_path,
        )?)?)
    } else {
        None
    };

    let base = Path::new(out_root).join("scout-to-backlog-candidates");
    fs::create_dir_all(&base)?;
    let run_dir = unique_dir(&base)?;
    fs::create_dir_all(run_dir.join("candidates"))?;

    let mut seen = 0usize;
    let mut converted = 0usize;
    let mut skipped_warning = 0usize;
    let mut candidates = Vec::new();

    for finding in &gate.findings {
        seen += 1;
        let convert = match finding.gate_status.as_str() {
            "approved" => true,
            "warn" => include_warnings,
            _ => false,
        };
        if !convert {
            if finding.gate_status == "warn" {
                skipped_warning += 1;
            }
            continue;
        }
        converted += 1;
        let hydrated = hydrate(finding, scout.as_ref());
        let candidate = build_candidate(finding, hydrated);
        fs::write(
            run_dir
                .join("candidates")
                .join(format!("{}.md", candidate.candidate_id)),
            markdown(&candidate),
        )?;
        candidates.push(candidate);
    }

    let artifact = CandidateArtifact {
        counts: CandidateCounts {
            seen,
            converted,
            skipped_warning,
        },
        candidates,
    };
    fs::write(
        run_dir.join("candidates.json"),
        serde_json::to_string_pretty(&artifact)?,
    )?;
    Ok(())
}

fn unique_dir(root: &Path) -> Result<PathBuf> {
    for attempt in 0..1000u32 {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let candidate = root.join(format!("{}-{}", stamp, attempt));
        if fs::create_dir(&candidate).is_ok() {
            return Ok(candidate);
        }
    }
    anyhow::bail!("unable to allocate unique run directory")
}

fn hydrate<'a>(
    gate: &'a GateFinding,
    scout: Option<&'a ScoutArtifact>,
) -> Option<&'a ScoutFinding> {
    let scout = scout?;
    if let Some(id) = &gate.id {
        if let Some(found) = scout
            .findings
            .iter()
            .find(|f| f.id.as_deref() == Some(id.as_str()))
        {
            return Some(found);
        }
    }
    gate.title.as_ref().and_then(|title| {
        scout
            .findings
            .iter()
            .find(|f| f.title.as_deref() == Some(title.as_str()))
    })
}

fn build_candidate(gate: &GateFinding, scout: Option<&ScoutFinding>) -> Candidate {
    let affected_files = gate
        .affected_files
        .clone()
        .or_else(|| scout.and_then(|s| s.affected_files.clone()))
        .unwrap_or_default();
    let evidence = gate
        .evidence
        .clone()
        .or_else(|| scout.and_then(|s| s.evidence.clone()))
        .unwrap_or_default();
    let acceptance_criteria = gate
        .suggested_acceptance_criteria
        .clone()
        .or_else(|| scout.and_then(|s| s.suggested_acceptance_criteria.clone()))
        .unwrap_or_default();
    let verification = gate
        .suggested_verification
        .clone()
        .or_else(|| scout.and_then(|s| s.suggested_verification.clone()))
        .unwrap_or_default();
    let risk_guess = gate
        .risk_guess
        .clone()
        .or_else(|| scout.and_then(|s| s.risk_guess.clone()))
        .unwrap_or_else(|| "unknown".into());

    let mut suggested_labels = Vec::new();
    if let Some(kind) = gate.finding_type.as_ref() {
        suggested_labels.push(format!("type:{}", kind));
    }
    suggested_labels.push(format!("risk:{}", risk_guess));
    suggested_labels.push("needs:human-review".to_string());

    Candidate {
        candidate_id: gate.id.clone().unwrap_or_else(|| "unknown".into()),
        source_gate_status: gate.gate_status.clone(),
        suggested_blueprint_phase: if gate.gate_status == "warn" {
            "needs:human".into()
        } else {
            "agent:ready".into()
        },
        provider_mutation_allowed: false,
        suggested_labels: suggested_labels
            .into_iter()
            .filter(|label| label != "agent:ready")
            .collect(),
        affected_files,
        evidence,
        acceptance_criteria,
        verification,
        hydration_used: scout.is_some(),
        hydration_match_method: if scout.is_some() {
            "id".into()
        } else {
            "none".into()
        },
        source_finding_path: gate
            .source_finding_path
            .clone()
            .or_else(|| scout.and_then(|s| s.finding_path.clone())),
        source_draft_issue_path: gate
            .source_draft_issue_path
            .clone()
            .or_else(|| scout.and_then(|s| s.draft_issue_path.clone())),
    }
}

fn markdown(candidate: &Candidate) -> String {
    format!(
        "# {}

status: {}
",
        candidate.candidate_id, candidate.source_gate_status
    )
}
