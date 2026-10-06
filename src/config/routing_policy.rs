use super::*;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub struct CandidateConfig {
    pub backend: String,
    /// Optional `routing.backend_instances` reference; absent preserves legacy behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_pool: Option<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub included_in_quota: bool,
    #[serde(default)]
    pub marginal_cost_usd: Option<f64>,
    #[serde(default)]
    pub quota_usage_percent: Option<f64>,
    #[serde(default)]
    pub quota_days_remaining: Option<f64>,
    /// Paid/API-backed candidates can remain configured as terminal
    /// fallbacks without ever being selected autonomously. An operator must
    /// grant a work-item-scoped route approval before routing may select one.
    #[serde(default)]
    pub requires_approval: bool,
}

/// A deterministic implementation-routing override selected from trusted
/// ticket metadata. Empty match lists are wildcards; every non-empty list
/// must match. Rules are evaluated in declaration order, so the first match
/// wins. Priority defines fallback tiers; equal-priority candidates are
/// balanced from runtime usage, with declaration order as the final tie-break.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub struct TaskRoutingRule {
    #[serde(default)]
    pub modes: Vec<String>,
    #[serde(default)]
    pub task_classes: Vec<String>,
    #[serde(default)]
    pub difficulties: Vec<String>,
    #[serde(default)]
    pub risks: Vec<String>,
    #[serde(default)]
    pub candidates: Vec<CandidateConfig>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct RoutingPolicy {
    /// Profile entries replace same-key global/default declarations.
    #[serde(default)]
    pub backend_instances: HashMap<String, BackendInstanceConfig>,
    /// Quota pool -> named credential whose account readings measure it, for
    /// example `vibe-monthly = "mistral-console"`. Readings only; the
    /// credential never enters execution. Profile entries replace same keys.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub quota_sources: HashMap<String, String>,
    #[serde(default)]
    pub default_backend: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub pm_backend: Option<String>,
    #[serde(default)]
    pub pm_model: Option<String>,
    #[serde(default)]
    pub improve_backend: Option<String>,
    #[serde(default)]
    pub improve_model: Option<String>,
    #[serde(default)]
    pub review_backend: Option<String>,
    #[serde(default)]
    pub review_model: Option<String>,
    #[serde(default)]
    pub strong_review_backend: Option<String>,
    #[serde(default)]
    pub strong_review_model: Option<String>,
    #[serde(default)]
    pub weak_review_backend: Option<String>,
    #[serde(default)]
    pub weak_review_model: Option<String>,
    /// Issue #123 / TICKET-118-stabilization: ROUTINE_REVIEWER -- the single
    /// STRONG first-line reviewer (e.g. Mistral-Medium via vibe). Replaces the
    /// deprecated `strong_review_backend`/`strong_review_model` pair; when set
    /// it is the authority used for ordinary review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine_reviewer: Option<CandidateConfig>,
    /// Issue #123: ESCALATORY_REVIEW -- an ORDERED LIST of advanced reviewers
    /// (Sonnet, Kimi, GLM, ...) used when routine review escalates. This is a
    /// list, not a single backend, which the old `weak_review_*` fields could
    /// not express. An escalatory reviewer is a more-capable model the pipeline
    /// escalates to and continues with (auto-merge eligible), distinct from the
    /// legacy `weak_review_*` safety-net that forced `human_required`.
    #[serde(default)]
    pub escalatory_reviewers: Vec<CandidateConfig>,
    /// TICKET-109: capabilities required for review, keyed by backend name
    /// (e.g. `{"claude": ["ponytail"]}`). Checked at preflight (TICKET-105)
    /// and activated in the review prompt -- missing a required capability
    /// is a hard stop, never a silent downgrade to an ordinary review.
    #[serde(default)]
    pub review_required_capabilities: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub pm_candidates: Option<Vec<CandidateConfig>>,
    #[serde(default)]
    pub improve_candidates: Option<Vec<CandidateConfig>>,
    #[serde(default)]
    pub pm_guidance_paths: Vec<String>,
    /// Ordered deterministic overrides for implementation work classified by
    /// trusted ticket metadata. Empty means no class-specific override.
    #[serde(default)]
    pub task_routing_rules: Vec<TaskRoutingRule>,
    #[serde(default)]
    pub review_candidates: Option<Vec<CandidateConfig>>,
    #[serde(default)]
    pub allow_review_fallback: bool,
    #[serde(default)]
    pub allow_implementation_fallback: bool,
    #[serde(default)]
    pub max_runs_per_backend_per_week: Option<u64>,
    #[serde(default)]
    pub max_runs_per_backend_per_session: Option<u64>,
    #[serde(default)]
    pub max_total_strong_model_runs_per_week: Option<u64>,
    #[serde(default)]
    pub max_total_strong_model_runs_per_session: Option<u64>,
    #[serde(default)]
    pub max_known_estimated_cost_per_week: Option<f64>,
    #[serde(default)]
    pub max_known_actual_cost_per_week: Option<f64>,
    /// Maximum completed routine reviews; defaults to `max_fix_attempts_per_mr + 1`.
    /// Each explicitly configured escalatory backend/model retains one bounded
    /// attempt beyond this cap before human escalation.
    #[serde(default)]
    pub max_review_cycles_per_ticket: Option<u32>,
    /// Maximum post-review repair dispatches for one MR before a human must
    /// intervene. A `NEEDS_FIX` verdict always gets this repair budget before
    /// it can become a human-required state. Unset defaults to two repairs.
    #[serde(default)]
    pub max_fix_attempts_per_mr: Option<u32>,
    /// Maximum paid/API-backed reviews for one work item. Unset defaults to
    /// three; quota-backed and local reviews do not consume this cap.
    #[serde(default)]
    pub max_paid_reviews_per_ticket: Option<u32>,
    /// Maximum genuine implementation failures before the controller gives
    /// up. This is deliberately separate from the post-review repair budget:
    /// a multi-model implementation ladder needs enough room to try each
    /// configured subscription candidate before human handoff. Defaults to 8.
    #[serde(default)]
    pub max_implementation_failures_per_ticket: Option<u32>,
    /// TICKET-127/Issue #124: per-repo merge policy gating what the
    /// controller does for a `READY_FOR_HUMAN` MR whose CI has been evaluated.
    /// `None` inherits the canonical/defaults policy (resolved to `Auto`).
    #[serde(default)]
    pub merge_policy: Option<MergePolicy>,
    /// Hold an APPROVE that changes a persisted or wire contract without
    /// compatibility evidence for a human. Unset means on (#1405).
    #[serde(default)]
    pub hold_contract_changes_for_human_review: Option<bool>,
}

impl RoutingPolicy {
    pub fn labeled_candidates(&self) -> Vec<(&'static str, &CandidateConfig)> {
        let mut c = Vec::new();
        let mut add = |l, v| c.push((l, v));
        if let Some(ref x) = self.routine_reviewer {
            add("routine_reviewer", x);
        }
        for x in &self.escalatory_reviewers {
            add("escalatory_reviewer", x);
        }
        for x in self.pm_candidates.iter().flatten() {
            add("pm_candidate", x);
        }
        for x in self.improve_candidates.iter().flatten() {
            add("improve_candidate", x);
        }
        for x in self.task_routing_rules.iter().flat_map(|r| &r.candidates) {
            add("task_routing_rule", x);
        }
        for x in self.review_candidates.iter().flatten() {
            add("review_candidate", x);
        }
        c
    }

    pub fn merged_with_defaults(&self, defaults: &RoutingPolicy) -> RoutingPolicy {
        let mut routing = merge_routing_policy(defaults.clone(), self.clone());
        routing.normalize_subscription_candidates(|id| {
            crate::credentials::get(id).is_ok_and(|info| {
                info.kind == crate::credentials::CredentialKind::ClaudeSubscriptionToken
            })
        });
        routing
    }

    /// `canonical` quota sources overridden by this policy's same-key entries.
    pub(crate) fn merged_quota_sources(
        &self,
        canonical: HashMap<String, String>,
    ) -> HashMap<String, String> {
        canonical
            .into_iter()
            .chain(self.quota_sources.clone())
            .collect()
    }

    /// Issue #123: resolve the effective ROUTINE_REVIEWER (STRONG tier).
    ///
    /// Prefers the new `routine_reviewer` field; falls back to the deprecated
    /// `strong_review_backend`/`strong_review_model` pair so existing configs
    /// keep working unchanged. Returns `None` when no routine reviewer is
    /// declared (caller decides whether that is a hard error or a warning).
    pub fn effective_routine_reviewer(&self) -> Option<CandidateConfig> {
        if let Some(r) = &self.routine_reviewer {
            return Some(r.clone());
        }
        match (&self.strong_review_backend, &self.strong_review_model) {
            (Some(b), m) => Some(CandidateConfig {
                backend: b.clone(),
                model: m.clone(),
                ..CandidateConfig::default()
            }),
            _ => None,
        }
    }

    /// Issue #123: resolve the effective ESCALATORY_REVIEW list (ordered).
    ///
    /// Prefers the new `escalatory_reviewers` list; falls back to the
    /// deprecated single `weak_review_backend`/`weak_review_model` entry so
    /// existing configs (which used the weak tier as a one-entry escalatory
    /// cascade) keep working. Returns an empty list when nothing is declared.
    pub fn effective_escalatory_reviewers(&self) -> Vec<CandidateConfig> {
        if !self.escalatory_reviewers.is_empty() {
            return self.escalatory_reviewers.clone();
        }
        match (&self.weak_review_backend, &self.weak_review_model) {
            (Some(b), m) => vec![CandidateConfig {
                backend: b.clone(),
                model: m.clone(),
                ..CandidateConfig::default()
            }],
            _ => vec![],
        }
    }

    #[allow(dead_code)] // enforced by dispatch review budget checks (#113)
    pub fn max_review_cycles_per_ticket(&self) -> u32 {
        self.max_review_cycles_per_ticket
            .unwrap_or_else(|| self.max_fix_attempts_per_mr().saturating_add(1))
    }

    pub fn max_fix_attempts_per_mr(&self) -> u32 {
        self.max_fix_attempts_per_mr.unwrap_or(2)
    }

    #[allow(dead_code)] // enforced by dispatch review budget checks (#113)
    pub fn max_paid_reviews_per_ticket(&self) -> u32 {
        self.max_paid_reviews_per_ticket.unwrap_or(3)
    }

    pub fn hold_contract_changes_for_human_review(&self) -> bool {
        self.hold_contract_changes_for_human_review.unwrap_or(true)
    }

    pub fn max_implementation_failures_per_ticket(&self) -> u32 {
        self.max_implementation_failures_per_ticket.unwrap_or(8)
    }
    pub fn find_quota_pool(
        &self,
        mode: &str,
        backend: &str,
        model: Option<&str>,
    ) -> Option<String> {
        let candidates = match JobKind::parse(mode).map(|kind| kind.family()) {
            Ok(JobFamily::Pm) => self.pm_candidates.as_ref(),
            Ok(JobFamily::Review) => self.review_candidates.as_ref(),
            Ok(JobFamily::ImproveLike) => self.improve_candidates.as_ref(),
            _ => None,
        };
        let configured = candidates.and_then(|list| {
            list.iter()
                .find(|c| c.backend == backend && c.model.as_deref() == model)
                .and_then(|c| c.quota_pool.as_deref())
        });
        crate::availability::resolve_candidate_quota_pool(backend, model, configured)
    }
}

impl Profile {
    pub fn effective_routing(&self, defaults: &Defaults) -> RoutingPolicy {
        self.routing.merged_with_defaults(&defaults.routing)
    }

    pub fn max_parallel_workers(&self) -> u32 {
        self.max_parallel_workers.unwrap_or(1).max(1)
    }

    pub fn max_open_managed_mrs(&self) -> u32 {
        self.max_open_managed_mrs
            .unwrap_or_else(|| self.max_parallel_workers())
            .max(1)
    }

    pub fn provider_kind(&self) -> Result<ProviderKind, UnknownProviderKind> {
        ProviderKind::parse(&self.provider)
    }
    pub fn pat(&self) -> String {
        match self.provider_kind() {
            Ok(ProviderKind::Gitlab) => std::env::var("GITLAB_PAT2")
                .or_else(|_| std::env::var("GITLAB_PAT"))
                .unwrap_or_default(),
            Ok(ProviderKind::Github) => std::env::var("GITHUB_TOKEN")
                .or_else(|_| std::env::var("GH_TOKEN"))
                .unwrap_or_default(),
            Err(_) => String::new(),
        }
    }

    pub fn pat_env_names(&self) -> &'static [&'static str] {
        match self.provider_kind() {
            Ok(ProviderKind::Gitlab) => &["GITLAB_PAT2", "GITLAB_PAT"],
            Ok(ProviderKind::Github) => &["GITHUB_TOKEN", "GH_TOKEN"],
            Err(_) => &[],
        }
    }

    pub fn provider_cli(&self) -> Option<&'static str> {
        match self.provider_kind() {
            Ok(ProviderKind::Gitlab) => Some("glab"),
            Ok(ProviderKind::Github) => Some("gh"),
            Err(_) => None,
        }
    }

    /// Build push URL without embedding PAT (auth is via GIT_ASKPASS).
    pub fn push_url(&self) -> Result<String> {
        match self.provider_kind() {
            Ok(ProviderKind::Gitlab) => {
                let base = self.gitlab_push_base()?;
                Ok(format!("{}/{}", base, normalize_repo_path(&self.repo)))
            }
            Ok(ProviderKind::Github) => Ok(format!(
                "https://github.com/{}",
                normalize_repo_path(&self.repo)
            )),
            Err(_) => Ok(self.repo.clone()),
        }
    }

    /// Human-facing repo URL (unlike `push_url()`, no oauth2@ placeholder).
    pub fn web_url(&self) -> Option<String> {
        match self.provider_kind() {
            Ok(ProviderKind::Github) => Some(format!(
                "https://github.com/{}",
                self.repo.trim_matches('/')
            )),
            Ok(ProviderKind::Gitlab) => {
                let base = self.gitlab_push_base().ok()?;
                let host = base.split_once('@').map(|(_, host)| host).unwrap_or(&base);
                Some(format!("https://{}/{}", host, self.repo.trim_matches('/')))
            }
            Err(_) => None,
        }
    }

    fn gitlab_push_base(&self) -> Result<String> {
        let base = self
            .provider_api_base
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("profile missing provider_api_base for gitlab"))?
            .trim();
        if base.is_empty() {
            anyhow::bail!("profile missing provider_api_base for gitlab");
        }

        let trimmed = base.trim_end_matches('/');
        let without_api = trimmed.strip_suffix("/api/v4").unwrap_or(trimmed);
        let (scheme, rest) = without_api
            .split_once("://")
            .unwrap_or(("https", without_api));
        let host = rest.split('/').next().unwrap_or("").trim_matches('/');
        if host.is_empty() {
            anyhow::bail!("invalid provider_api_base for gitlab: {}", base);
        }
        Ok(format!("{}://oauth2@{}", scheme, host))
    }
}

/// Field-level merge: `repo`'s own explicit values win; unset fields
/// inherit from `canonical`. Candidate lists (Vec) replace wholesale when
/// the repo sets them (not concatenate); the capability map merges by key
/// so a repo declaring one backend's capabilities doesn't erase another
/// backend's canonical-declared ones.
pub(super) fn merge_routing_policy(
    canonical: RoutingPolicy,
    mut repo: RoutingPolicy,
) -> RoutingPolicy {
    repo.backend_instances =
        backend_instances::merge_instance_maps(canonical.backend_instances, repo.backend_instances);
    repo.default_backend = repo.default_backend.or(canonical.default_backend);
    repo.default_model = repo.default_model.or(canonical.default_model);
    repo.pm_backend = repo.pm_backend.or(canonical.pm_backend);
    repo.pm_model = repo.pm_model.or(canonical.pm_model);
    repo.improve_backend = repo.improve_backend.or(canonical.improve_backend);
    repo.improve_model = repo.improve_model.or(canonical.improve_model);
    repo.review_backend = repo.review_backend.or(canonical.review_backend);
    repo.review_model = repo.review_model.or(canonical.review_model);
    repo.strong_review_backend = repo
        .strong_review_backend
        .or(canonical.strong_review_backend);
    repo.strong_review_model = repo.strong_review_model.or(canonical.strong_review_model);
    repo.weak_review_backend = repo.weak_review_backend.or(canonical.weak_review_backend);
    repo.weak_review_model = repo.weak_review_model.or(canonical.weak_review_model);
    repo.routine_reviewer = repo.routine_reviewer.or(canonical.routine_reviewer);
    if repo.escalatory_reviewers.is_empty() {
        repo.escalatory_reviewers = canonical.escalatory_reviewers.clone();
    }
    repo.pm_candidates = repo.pm_candidates.or(canonical.pm_candidates);
    repo.improve_candidates = repo.improve_candidates.or(canonical.improve_candidates);
    if repo.pm_guidance_paths.is_empty() {
        repo.pm_guidance_paths = canonical.pm_guidance_paths;
    }
    if repo.task_routing_rules.is_empty() {
        repo.task_routing_rules = canonical.task_routing_rules.clone();
    }
    repo.review_candidates = repo.review_candidates.or(canonical.review_candidates);
    repo.allow_review_fallback = repo.allow_review_fallback || canonical.allow_review_fallback;
    repo.allow_implementation_fallback =
        repo.allow_implementation_fallback || canonical.allow_implementation_fallback;
    repo.max_runs_per_backend_per_week = repo
        .max_runs_per_backend_per_week
        .or(canonical.max_runs_per_backend_per_week);
    repo.max_runs_per_backend_per_session = repo
        .max_runs_per_backend_per_session
        .or(canonical.max_runs_per_backend_per_session);
    repo.max_total_strong_model_runs_per_week = repo
        .max_total_strong_model_runs_per_week
        .or(canonical.max_total_strong_model_runs_per_week);
    repo.max_total_strong_model_runs_per_session = repo
        .max_total_strong_model_runs_per_session
        .or(canonical.max_total_strong_model_runs_per_session);
    repo.max_known_estimated_cost_per_week = repo
        .max_known_estimated_cost_per_week
        .or(canonical.max_known_estimated_cost_per_week);
    repo.max_known_actual_cost_per_week = repo
        .max_known_actual_cost_per_week
        .or(canonical.max_known_actual_cost_per_week);
    repo.max_review_cycles_per_ticket = repo
        .max_review_cycles_per_ticket
        .or(canonical.max_review_cycles_per_ticket);
    repo.max_fix_attempts_per_mr = repo
        .max_fix_attempts_per_mr
        .or(canonical.max_fix_attempts_per_mr);
    repo.max_paid_reviews_per_ticket = repo
        .max_paid_reviews_per_ticket
        .or(canonical.max_paid_reviews_per_ticket);
    repo.max_implementation_failures_per_ticket = repo
        .max_implementation_failures_per_ticket
        .or(canonical.max_implementation_failures_per_ticket);
    repo.quota_sources = repo.merged_quota_sources(canonical.quota_sources);
    let mut capabilities = canonical.review_required_capabilities;
    capabilities.extend(repo.review_required_capabilities);
    repo.review_required_capabilities = capabilities;
    repo.merge_policy = repo.merge_policy.or(canonical.merge_policy);
    repo.hold_contract_changes_for_human_review = repo
        .hold_contract_changes_for_human_review
        .or(canonical.hold_contract_changes_for_human_review);
    repo
}
