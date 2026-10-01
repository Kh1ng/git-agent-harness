//! Seeds a project's existing memory and handoff docs into the memory
//! gateway, so chat has continuity from day one (`GAH_IMPORT_REPO` at
//! install). Uses /capture, not /seed: /seed writes into a disposable
//! per-call directory that /recall never reads (issue #884).

use super::files::env_get;
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

#[derive(clap::Args, Debug, Clone)]
pub struct Args {
    /// The project checkout to read docs from.
    #[arg(long)]
    pub repo: PathBuf,
    /// e.g. gah:manager:github.com/org/repo
    #[arg(long)]
    pub session_key: String,
    /// Comma-separated paths relative to --repo. Default: MANAGER_MEMORY.md,
    /// MEMORY.md, and docs/*handoff*.md.
    #[arg(long)]
    pub docs: Option<String>,
    /// Default: $TDAI_GATEWAY_URL, or http://127.0.0.1:8420.
    #[arg(long)]
    pub gateway_url: Option<String>,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub limit: Option<usize>,
}

const CONVENTIONAL_DOCS: [&str; 3] = ["docs/MANAGER_MEMORY.md", "MANAGER_MEMORY.md", "MEMORY.md"];

pub fn discover(repo: &Path) -> Vec<String> {
    let mut found: Vec<String> = CONVENTIONAL_DOCS
        .iter()
        .filter(|rel| repo.join(rel).is_file())
        .map(|rel| rel.to_string())
        .collect();
    let mut handoffs: Vec<String> = std::fs::read_dir(repo.join("docs"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".md") && name.to_lowercase().contains("handoff"))
        .map(|name| format!("docs/{name}"))
        .collect();
    handoffs.sort();
    for rel in handoffs {
        if !found.contains(&rel) {
            found.push(rel);
        }
    }
    found
}

/// Framed as a real question and answer: the gateway's extractor summarizes
/// conversation, and an "import this file" instruction produced a shallow
/// summary on the first backfill.
pub fn capture_body(session_key: &str, project: &str, relative: &str, content: &str) -> String {
    serde_json::json!({
        "user_content": format!("What's the current state of the {project} project, per {relative}?"),
        "assistant_content": content,
        "session_key": session_key,
    })
    .to_string()
}

pub fn run(args: &Args, home: &Path) -> Result<()> {
    let gateway = args
        .gateway_url
        .clone()
        .or_else(|| {
            std::env::var("TDAI_GATEWAY_URL")
                .ok()
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_else(|| "http://127.0.0.1:8420".into());
    let repo = std::fs::canonicalize(&args.repo).unwrap_or_else(|_| args.repo.clone());
    let project = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let relatives: Vec<String> = match &args.docs {
        Some(list) => list
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect(),
        None => discover(&repo),
    };
    let mut docs = Vec::new();
    for rel in relatives {
        match std::fs::read(repo.join(&rel)) {
            Err(_) => eprintln!("skip (missing): {rel}"),
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).trim().to_string();
                if text.is_empty() {
                    eprintln!("skip (empty): {rel}");
                } else {
                    docs.push((rel, text));
                }
            }
        }
    }
    if let Some(limit) = args.limit {
        docs.truncate(limit);
    }
    println!(
        "Gateway: {gateway}\nSession key: {}\nDocs to seed: {}\nMode: {}\n",
        args.session_key,
        docs.len(),
        if args.dry_run { "DRY RUN" } else { "LIVE" }
    );
    if docs.is_empty() {
        bail!("Nothing to seed: no matching docs found. Pass --docs to name them.");
    }
    let key = env_get(
        &home.join(".config/gah/tdai-gateway.env"),
        "TDAI_GATEWAY_API_KEY",
    );
    let url = format!("{}/capture", gateway.trim_end_matches('/'));
    let mut seeded = 0;
    for (index, (rel, text)) in docs.iter().enumerate() {
        if args.dry_run {
            println!("[dry-run] {rel}: {} chars", text.chars().count());
            seeded += 1;
            continue;
        }
        if index > 0 {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        let body = capture_body(&args.session_key, &project, rel, text);
        match crate::curl_http::request("POST", &url, Some(&body), key.as_deref(), 60) {
            Ok(response) if response.status == 200 => {
                let recorded = serde_json::from_slice::<serde_json::Value>(&response.body)
                    .ok()
                    .and_then(|value| value.get("l0_recorded").cloned())
                    .unwrap_or_default();
                println!("{rel}: ok (l0={recorded})");
                seeded += 1;
            }
            Ok(response) => println!(
                "{rel}: FAILED (HTTP {}: {})",
                response.status,
                String::from_utf8_lossy(&response.body)
            ),
            Err(error) => println!("{rel}: FAILED ({error})"),
        }
    }
    println!("\nDone. seeded={seeded}/{}", docs.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conventional_and_handoff_docs_are_found_once_in_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        for file in [
            "MEMORY.md",
            "docs/MANAGER_MEMORY.md",
            "docs/Z-Handoff.md",
            "docs/a_handoff.md",
            "docs/notes.md",
            "docs/handoff.txt",
        ] {
            std::fs::write(dir.path().join(file), "x").unwrap();
        }
        assert_eq!(
            discover(dir.path()),
            [
                "docs/MANAGER_MEMORY.md",
                "MEMORY.md",
                "docs/Z-Handoff.md",
                "docs/a_handoff.md"
            ]
        );
    }

    #[test]
    fn a_doc_is_captured_as_a_question_and_its_answer() {
        let body: serde_json::Value =
            serde_json::from_str(&capture_body("gah:manager:x", "proj", "MEMORY.md", "state"))
                .unwrap();
        assert_eq!(
            body["user_content"],
            "What's the current state of the proj project, per MEMORY.md?"
        );
        assert_eq!(body["assistant_content"], "state");
        assert_eq!(body["session_key"], "gah:manager:x");
    }
}
