use crate::models::{
    Candidate, CandidateArtifact, CandidateCounts, GateArtifact, GateFinding, ScoutArtifact,
};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn run(gate_artifact: &str, include_warnings: bool, out_root: &str) -> Result<()> {
    let gate_path = Path::new(gate_artifact).join("gate.json");
    let gate: GateArtifact = serde_json::from_str(&fs::read_to_string(&gate_path)?)?;
    let scout = load_scout(gate.source_scout_artifact.as_deref())?;

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
        let (hydrated, is_hydrated) = hydrate_finding(finding, scout.as_ref());
        let candidate = build_candidate(finding, &hydrated, is_hydrated);
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

fn load_scout(path: Option<&str>) -> Result<Option<ScoutArtifact>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let scout_path = Path::new(path).join("scout.json");
    Ok(Some(serde_json::from_str(&fs::read_to_string(
        scout_path,
    )?)?))
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

fn hydrate_finding(gate: &GateFinding, scout: Option<&ScoutArtifact>) -> (GateFinding, bool) {
    let Some(scout) = scout else {
        return (gate.clone(), false);
    };
    let matched = match gate.id.as_ref() {
        Some(id) => scout
            .findings
            .iter()
            .find(|f| f.id.as_deref() == Some(id.as_str())),
        None => None,
    }
    .or_else(|| {
        gate.title.as_ref().and_then(|title| {
            scout
                .findings
                .iter()
                .find(|f| f.title.as_deref() == Some(title.as_str()))
        })
    });
    let Some(scout_finding) = matched else {
        return (gate.clone(), false);
    };

    let mut hydrated = gate.clone();
    merge_missing(&mut hydrated.affected_files, scout_finding.affected_files.clone());
    merge_missing(&mut hydrated.evidence, scout_finding.evidence.clone());
    merge_missing(&mut hydrated.commands, scout_finding.commands.clone());
    merge_missing(
        &mut hydrated.suggested_acceptance_criteria,
        scout_finding.suggested_acceptance_criteria.clone(),
    );
    merge_missing(
        &mut hydrated.suggested_verification,
        scout_finding.suggested_verification.clone(),
    );
    merge_missing(&mut hydrated.risk_guess, scout_finding.risk_guess.clone());
    merge_missing(&mut hydrated.confidence, scout_finding.confidence.clone());
    merge_missing(&mut hydrated.likely_agent_safe, scout_finding.likely_agent_safe);
    merge_missing(&mut hydrated.finding_path, scout_finding.finding_path.clone());
    merge_missing(
        &mut hydrated.draft_issue_path,
        scout_finding.draft_issue_path.clone(),
    );
    (hydrated, true)
}

fn merge_missing<T>(target: &mut Option<T>, fallback: Option<T>) {
    if target.is_none() {
        *target = fallback;
    }
}

fn build_candidate(gate: &GateFinding, hydrated: &GateFinding, is_hydrated: bool) -> Candidate {
    let source = hydrated;
    let mut suggested_labels = Vec::new();
    if let Some(kind) = source.finding_type.as_ref() {
        suggested_labels.push(format!("type:{}", kind));
    }
    if let Some(risk) = source.risk_guess.as_ref() {
        suggested_labels.push(format!("risk:{}", risk));
    } else {
        suggested_labels.push("risk:unknown".to_string());
    }
    suggested_labels.push("needs:human-review".to_string());

    Candidate {
        candidate_id: source.id.clone().unwrap_or_else(|| "unknown".into()),
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
        affected_files: source.affected_files.clone().unwrap_or_default(),
        evidence: source.evidence.clone().unwrap_or_default(),
        acceptance_criteria: source
            .suggested_acceptance_criteria
            .clone()
            .unwrap_or_default(),
        verification: source.suggested_verification.clone().unwrap_or_default(),
        hydration_used: is_hydrated,
        hydration_source: if is_hydrated { "scout.json".into() } else { "gate.json".into() },
        hydration_match_method: if is_hydrated { "id".into() } else { "none".into() },
        hydrated_fields: hydrated_fields(gate, source, is_hydrated),
        debug_gate_keys: gate_keys(gate),
        debug_scout_keys: scout_keys(source, is_hydrated),
        debug_hydrated_keys: hydrated_keys(source),
        debug_hydrated_finding_excerpt: hydrated_excerpt(source),
        source_finding_path: gate
            .source_finding_path
            .clone()
            .or_else(|| source.finding_path.clone()),
        source_draft_issue_path: gate
            .source_draft_issue_path
            .clone()
            .or_else(|| source.draft_issue_path.clone()),
    }
}

fn hydrated_fields(gate: &GateFinding, source: &GateFinding, hydrated: bool) -> Vec<String> {
    if !hydrated {
        return Vec::new();
    }
    let mut fields = Vec::new();
    for (name, changed) in [
        (
            "affected_files",
            gate.affected_files.is_none() && source.affected_files.is_some(),
        ),
        (
            "evidence",
            gate.evidence.is_none() && source.evidence.is_some(),
        ),
        (
            "commands",
            gate.commands.is_none() && source.commands.is_some(),
        ),
        (
            "suggested_acceptance_criteria",
            gate.suggested_acceptance_criteria.is_none()
                && source.suggested_acceptance_criteria.is_some(),
        ),
        (
            "suggested_verification",
            gate.suggested_verification.is_none() && source.suggested_verification.is_some(),
        ),
        (
            "risk_guess",
            gate.risk_guess.is_none() && source.risk_guess.is_some(),
        ),
        (
            "confidence",
            gate.confidence.is_none() && source.confidence.is_some(),
        ),
        (
            "likely_agent_safe",
            gate.likely_agent_safe.is_none() && source.likely_agent_safe.is_some(),
        ),
        (
            "finding_path",
            gate.finding_path.is_none() && source.finding_path.is_some(),
        ),
        (
            "draft_issue_path",
            gate.draft_issue_path.is_none() && source.draft_issue_path.is_some(),
        ),
    ] {
        if changed {
            fields.push(name.to_string());
        }
    }
    fields
}

fn gate_keys(gate: &GateFinding) -> Vec<String> {
    let mut keys = vec![
        "id".to_string(),
        "title".to_string(),
        "type".to_string(),
        "gate_status".to_string(),
    ];
    if gate.source_finding_path.is_some() {
        keys.push("source_finding_path".to_string());
    }
    if gate.source_draft_issue_path.is_some() {
        keys.push("source_draft_issue_path".to_string());
    }
    keys
}

fn scout_keys(source: &GateFinding, hydrated: bool) -> Vec<String> {
    if !hydrated {
        return Vec::new();
    }
    let mut keys = Vec::new();
    if source.affected_files.is_some() {
        keys.push("affected_files".to_string());
    }
    if source.evidence.is_some() {
        keys.push("evidence".to_string());
    }
    if source.commands.is_some() {
        keys.push("commands".to_string());
    }
    if source.suggested_acceptance_criteria.is_some() {
        keys.push("suggested_acceptance_criteria".to_string());
    }
    if source.suggested_verification.is_some() {
        keys.push("suggested_verification".to_string());
    }
    keys
}

fn hydrated_keys(source: &GateFinding) -> Vec<String> {
    let mut keys = Vec::new();
    if source.affected_files.is_some() {
        keys.push("affected_files".to_string());
    }
    if source.evidence.is_some() {
        keys.push("evidence".to_string());
    }
    if source.commands.is_some() {
        keys.push("commands".to_string());
    }
    if source.suggested_acceptance_criteria.is_some() {
        keys.push("suggested_acceptance_criteria".to_string());
    }
    if source.suggested_verification.is_some() {
        keys.push("suggested_verification".to_string());
    }
    if source.risk_guess.is_some() {
        keys.push("risk_guess".to_string());
    }
    if source.confidence.is_some() {
        keys.push("confidence".to_string());
    }
    if source.likely_agent_safe.is_some() {
        keys.push("likely_agent_safe".to_string());
    }
    if source.finding_path.is_some() {
        keys.push("finding_path".to_string());
    }
    if source.draft_issue_path.is_some() {
        keys.push("draft_issue_path".to_string());
    }
    keys
}

fn hydrated_excerpt(source: &GateFinding) -> String {
    format!(
        "{}|{}|{}",
        source.id.clone().unwrap_or_default(),
        source.title.clone().unwrap_or_default(),
        source.gate_status
    )
}

fn markdown(candidate: &Candidate) -> String {
    format!(
        "# {}\n\nstatus: {}\n",
        candidate.candidate_id, candidate.source_gate_status
    )
}
