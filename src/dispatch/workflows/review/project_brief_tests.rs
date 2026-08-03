use super::build_review_project_brief_section;
use crate::config;
use std::collections::HashMap;
use tempfile::tempdir;

#[test]
fn review_project_brief_section_is_injected_bounded_in_invariant_order() {
    let mut cfg = config::GahConfig {
        context: Default::default(),
        defaults: config::Defaults::default(),
        profiles: HashMap::new(),
    };
    cfg.context.profiles.insert(
        "gah".into(),
        crate::context::ContextOverride {
            include_review_project_brief: Some(true),
            ..Default::default()
        },
    );

    let tmp = tempdir().unwrap();
    let mut profile = crate::config::tests::test_profile_for_notifications();
    profile.local_path = tmp.path().display().to_string();
    std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
    std::fs::write(
        tmp.path().join("docs/PROJECT_BRIEF.md"),
        format!("## Project heading\n{}", "x".repeat(5_000)),
    )
    .unwrap();

    let (section, metadata) = build_review_project_brief_section(&cfg, "gah", &profile);
    let metadata = metadata.unwrap();
    let full_prompt = format!("## Review Pack\n\n{}\n## Diff\n{}", section, "diff");

    assert!(metadata.included);
    assert!(metadata.truncated);
    assert!(metadata.source_bytes > 4_096);
    assert!(metadata.sent_bytes <= 4_096);
    assert_eq!(metadata.source_hash.as_ref().unwrap().len(), 64);
    assert!(section.contains("## Project Brief"));
    assert!(section.contains("  ## Project heading"));
    assert!(!section.contains("\n## Project heading"));
    assert!(full_prompt.find("## Project Brief").unwrap() < full_prompt.find("## Diff").unwrap());
    assert!(full_prompt.contains("[Project Brief truncated at 4096 bytes"));
}

#[test]
fn review_project_brief_section_respects_profile_gate() {
    let cfg = config::GahConfig {
        context: Default::default(),
        defaults: config::Defaults::default(),
        profiles: HashMap::new(),
    };
    let profile = crate::config::tests::test_profile_for_notifications();

    let (section, metadata) = build_review_project_brief_section(&cfg, "gah", &profile);

    assert!(section.is_empty());
    assert!(metadata.is_none());
}
