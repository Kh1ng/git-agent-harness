use super::*;

#[test]
fn reviewer_tier_allow_listed_reviewer_is_strong() {
    let tmp = tempfile::tempdir().unwrap();
    let mut prof = profile(tmp.path());
    prof.routing.allowed_models.insert(
        "review".into(),
        vec![crate::config::CandidateConfig {
            backend: "claude".into(),
            model: Some("opus".into()),
            ..Default::default()
        }],
    );
    let cfg = gah_config(RoutingPolicy::default());

    assert_eq!(
        derive_reviewer_tier(&cfg, &prof, &route_decision("claude", Some("opus"), false)),
        ReviewerTier::Strong
    );
    assert_eq!(
        derive_reviewer_tier(&cfg, &prof, &route_decision("codex", Some("gpt-4"), false)),
        ReviewerTier::Standard
    );
}
