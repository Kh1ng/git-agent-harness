use super::*;
use crate::config::RoutingPolicy;
use crate::dispatch::test_util::{gah_config_with_ledger, profile};
use crate::models::PmPlan;
use std::collections::HashMap;

#[derive(Default)]
struct FakePublisher {
    issues: HashMap<String, ProviderIssue>,
    labels: Vec<String>,
    create_calls: usize,
    fail_create_call: Option<usize>,
    close_source_after_create: bool,
    child_links: Vec<(String, String)>,
    dependency_links: Vec<(String, String)>,
}

impl FakePublisher {
    fn with_source() -> Self {
        let mut fake = Self::default();
        fake.issues.insert(
            "42".to_string(),
            ProviderIssue {
                id: "4200".to_string(),
                number: "42".to_string(),
                url: "https://provider.example/project/issues/42".to_string(),
                state: "open".to_string(),
                title: "Parent".to_string(),
                body: String::new(),
                labels: Vec::new(),
            },
        );
        fake
    }
}

impl IssuePublisher for FakePublisher {
    fn get_issue(&mut self, number: &str) -> Result<ProviderIssue> {
        self.issues
            .get(number)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing fake issue {number}"))
    }

    fn list_issues(&mut self) -> Result<Vec<ProviderIssue>> {
        Ok(self.issues.values().cloned().collect())
    }

    fn list_labels(&mut self) -> Result<Vec<String>> {
        Ok(self.labels.clone())
    }

    fn create_issue(
        &mut self,
        title: &str,
        body: &str,
        labels: &[String],
    ) -> Result<ProviderIssue> {
        self.create_calls += 1;
        if self.fail_create_call == Some(self.create_calls) {
            anyhow::bail!("injected provider create failure");
        }
        let number = (100 + self.create_calls).to_string();
        let issue = ProviderIssue {
            id: format!("id-{number}"),
            number: number.clone(),
            url: format!("https://provider.example/project/issues/{number}"),
            state: "open".to_string(),
            title: title.to_string(),
            body: body.to_string(),
            labels: labels.to_vec(),
        };
        self.issues.insert(number, issue.clone());
        if self.close_source_after_create {
            self.issues.get_mut("42").unwrap().state = "closed".to_string();
        }
        Ok(issue)
    }

    fn link_child(&mut self, parent_number: &str, child: &ProviderIssue) -> Result<()> {
        let edge = (parent_number.to_string(), child.number.clone());
        if !self.child_links.contains(&edge) {
            self.child_links.push(edge);
        }
        Ok(())
    }

    fn link_dependency(&mut self, dependency: &ProviderIssue, child: &ProviderIssue) -> Result<()> {
        let edge = (dependency.number.clone(), child.number.clone());
        if !self.dependency_links.contains(&edge) {
            self.dependency_links.push(edge);
        }
        Ok(())
    }
}

fn artifact(provider_profile: &Profile) -> PmPlanArtifact {
    let plan: PmPlan = serde_json::from_str(
        r#"{
          "title":"Plan",
          "summary":"Summary",
          "tickets":[
            {
              "key":"base","title":"Base child","summary":"Base summary",
              "objective":"Build base","task_class":"feature","difficulty":"easy",
              "risk":"low","execution_disposition":"autonomous",
              "recommended_routing":{"capability":"edit","min_tier":"standard"},
              "affected_areas":["core"],"affected_files":["src/base.rs"],
              "acceptance_criteria":["base works"],"verification_commands":["cargo test base"],
              "depends_on":[],"duplicate_evidence":["No matching issue"],
              "uncovered_reason":"No existing work covers it"
            },
            {
              "key":"dependent","title":"Dependent child","summary":"Dependent summary",
              "objective":"Build dependent","task_class":"feature","difficulty":"medium",
              "risk":"medium","execution_disposition":"human_required",
              "recommended_routing":{"capability":"edit","min_tier":"strong"},
              "affected_areas":["core"],"affected_files":["src/dependent.rs"],
              "acceptance_criteria":["dependent works"],"verification_commands":["cargo test dependent"],
              "depends_on":["base"],"duplicate_evidence":["No matching issue"],
              "uncovered_reason":"No existing work covers it"
            }
          ]
        }"#,
    )
    .unwrap();
    PmPlanArtifact {
        schema_version: 1,
        profile: provider_profile.display_name.clone(),
        repo: provider_profile.repo.clone(),
        target: "#42".to_string(),
        open_issue_count: 1,
        open_mr_count: 0,
        merged_mr_count: 0,
        ticket_count: plan.tickets.len(),
        plan,
    }
}

fn run_publish(
    tmp: &Path,
    provider_profile: &Profile,
    artifact: &PmPlanArtifact,
    state: &mut PublicationState,
    fake: &mut FakePublisher,
    dry_run: bool,
) -> Result<()> {
    let cfg = gah_config_with_ledger(tmp, RoutingPolicy::default());
    let fingerprint = plan_fingerprint(artifact)?;
    let order = topological_order(&artifact.plan.tickets)?;
    let state_path = tmp.join("publication.json");
    let context = PublishContext {
        cfg: &cfg,
        profile_name: "test",
        profile: provider_profile,
        artifact,
        source_issue_number: "42",
        fingerprint: &fingerprint,
        order: &order,
        state_path: &state_path,
    };
    publish_with_provider(&context, state, fake, dry_run)
}

fn initial_state(provider_profile: &Profile, artifact: &PmPlanArtifact) -> PublicationState {
    PublicationState {
        schema_version: 1,
        plan_fingerprint: plan_fingerprint(artifact).unwrap(),
        profile: "test".to_string(),
        repo: provider_profile.repo.clone(),
        source_issue_number: "42".to_string(),
        status: "planned".to_string(),
        children: BTreeMap::new(),
    }
}

#[test]
fn github_publication_is_native_idempotent_and_holds_owner_work() {
    let tmp = tempfile::tempdir().unwrap();
    let mut provider_profile = profile(tmp.path());
    provider_profile.provider = "github".to_string();
    provider_profile
        .publishing
        .pm_difficulty_labels
        .insert("easy".to_string(), "difficulty:easy".to_string());
    provider_profile.publishing.pm_execution_labels.insert(
        "human_required".to_string(),
        "exec:owner-decision".to_string(),
    );
    let artifact = artifact(&provider_profile);
    let mut state = initial_state(&provider_profile, &artifact);
    let mut fake = FakePublisher::with_source();
    fake.labels = vec![
        "exec:autonomous".to_string(),
        "difficulty:easy".to_string(),
        "exec:owner-decision".to_string(),
    ];

    run_publish(
        tmp.path(),
        &provider_profile,
        &artifact,
        &mut state,
        &mut fake,
        false,
    )
    .unwrap();
    assert_eq!(fake.create_calls, 2);
    assert_eq!(fake.child_links.len(), 2);
    assert_eq!(fake.dependency_links.len(), 1);
    assert!(fake.issues["101"]
        .labels
        .contains(&"exec:autonomous".to_string()));
    assert!(!fake.issues["102"]
        .labels
        .contains(&"exec:autonomous".to_string()));
    assert!(fake.issues["102"]
        .labels
        .contains(&"exec:owner-decision".to_string()));

    run_publish(
        tmp.path(),
        &provider_profile,
        &artifact,
        &mut state,
        &mut fake,
        false,
    )
    .unwrap();
    assert_eq!(fake.create_calls, 2, "rerun must not duplicate children");
}

#[test]
fn gitlab_partial_failure_resumes_without_duplicate_children() {
    let tmp = tempfile::tempdir().unwrap();
    let mut provider_profile = profile(tmp.path());
    provider_profile.provider = "gitlab".to_string();
    provider_profile.repo = "https://gitlab.example/group/project".to_string();
    provider_profile.provider_project_id = Some("77".to_string());
    let artifact = artifact(&provider_profile);
    let mut state = initial_state(&provider_profile, &artifact);
    let mut fake = FakePublisher::with_source();
    fake.labels = vec!["exec:autonomous".to_string()];
    fake.fail_create_call = Some(2);

    let error = run_publish(
        tmp.path(),
        &provider_profile,
        &artifact,
        &mut state,
        &mut fake,
        false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("creating PM child issue"));
    assert_eq!(state.children.len(), 1);

    fake.fail_create_call = None;
    run_publish(
        tmp.path(),
        &provider_profile,
        &artifact,
        &mut state,
        &mut fake,
        false,
    )
    .unwrap();
    assert_eq!(state.children.len(), 2);
    assert_eq!(
        fake.issues
            .values()
            .filter(|issue| issue.number != "42")
            .count(),
        2
    );
}

#[test]
fn dry_run_and_cross_profile_checks_never_write() {
    let tmp = tempfile::tempdir().unwrap();
    let provider_profile = profile(tmp.path());
    let mut artifact = artifact(&provider_profile);
    let mut state = initial_state(&provider_profile, &artifact);
    let mut fake = FakePublisher::with_source();
    fake.labels = vec!["exec:autonomous".to_string()];
    run_publish(
        tmp.path(),
        &provider_profile,
        &artifact,
        &mut state,
        &mut fake,
        true,
    )
    .unwrap();
    assert_eq!(fake.create_calls, 0);
    assert!(state.children.is_empty());

    artifact.repo = "other/project".to_string();
    let error = validate_publication_contract(&provider_profile, &artifact).unwrap_err();
    assert!(error.to_string().contains("project mismatch"));
}

#[test]
fn missing_configured_provider_label_fails_before_issue_creation() {
    let tmp = tempfile::tempdir().unwrap();
    let provider_profile = profile(tmp.path());
    let artifact = artifact(&provider_profile);
    let mut state = initial_state(&provider_profile, &artifact);
    let mut fake = FakePublisher::with_source();

    let error = run_publish(
        tmp.path(),
        &provider_profile,
        &artifact,
        &mut state,
        &mut fake,
        false,
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("does not exist in provider project"));
    assert_eq!(fake.create_calls, 0);
}

#[test]
fn publication_stops_when_source_closes_before_a_write() {
    let tmp = tempfile::tempdir().unwrap();
    let provider_profile = profile(tmp.path());
    let artifact = artifact(&provider_profile);
    let mut state = initial_state(&provider_profile, &artifact);
    let mut fake = FakePublisher::with_source();
    fake.labels = vec!["exec:autonomous".to_string()];
    fake.close_source_after_create = true;
    let error = run_publish(
        tmp.path(),
        &provider_profile,
        &artifact,
        &mut state,
        &mut fake,
        false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("publication stopped"));
    assert_eq!(fake.create_calls, 1);
    assert_eq!(state.children.len(), 1);
    assert!(
        fake.child_links.is_empty(),
        "closed parent must not be mutated"
    );
}

#[test]
fn dependency_order_is_deterministic_and_rejects_cycles() {
    let tmp = tempfile::tempdir().unwrap();
    let provider_profile = profile(tmp.path());
    let mut artifact = artifact(&provider_profile);
    assert_eq!(
        topological_order(&artifact.plan.tickets).unwrap(),
        vec![0, 1]
    );
    artifact.plan.tickets[0].key = " base ".to_string();
    artifact.plan.tickets[1].depends_on = vec![" base ".to_string()];
    assert_eq!(
        topological_order(&artifact.plan.tickets).unwrap(),
        vec![0, 1],
        "validated plan keys and dependencies use their canonical trimmed identity"
    );
    artifact.plan.tickets[0].depends_on = vec!["dependent".to_string()];
    assert!(topological_order(&artifact.plan.tickets).is_err());
}
