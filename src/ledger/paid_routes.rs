//! Work-item paid-route requests and grants, projected from the authoritative
//! ledger. HTTP clients consume this view rather than interpreting control rows.

use super::{
    active_paid_route_approval_destinations_from_entries, effective_human_gate_from_entries,
    work_id_aliases, LedgerEntry,
};
use crate::config::{Defaults, Profile};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize)]
pub struct PaidRouteApproval {
    pub profile: String,
    pub work_id: String,
    pub backend: String,
    pub backend_instance: Option<String>,
    pub model: Option<String>,
    pub approved: bool,
    pub requested: bool,
}

fn canonical_work_id(work_id: &str) -> String {
    work_id_aliases(work_id)
        .into_iter()
        .find(|id| id.starts_with('#'))
        .unwrap_or_else(|| work_id.to_string())
}

/// Show exact pending policy requests and existing grants for a profile/repo.
/// Grants remain visible after a gate clears so an operator can revoke them.
/// No routing probes, provider calls, or configuration writes occur here.
pub fn paid_route_approvals_from_entries(
    entries: &[LedgerEntry],
    profile_name: &str,
    profile: &Profile,
    defaults: &Defaults,
) -> Vec<PaidRouteApproval> {
    let routing = profile.effective_routing(defaults);
    let entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.profile == profile_name && entry.repo_id == profile.repo_id)
        .cloned()
        .collect();
    let work_ids: BTreeSet<_> = entries
        .iter()
        .filter_map(|entry| entry.work_id.as_deref())
        .map(canonical_work_id)
        .collect();
    let mut result = Vec::new();
    for work_id in work_ids {
        let active =
            active_paid_route_approval_destinations_from_entries(&entries, profile_name, &work_id);
        let mut routes = BTreeMap::new();
        for entry in &entries {
            if entry.mode != "paid_route_approval_grant"
                || entry.work_id.as_deref().map(canonical_work_id).as_deref() != Some(&work_id)
            {
                continue;
            }
            let key = (
                entry
                    .usage
                    .backend_instance
                    .clone()
                    .unwrap_or_else(|| entry.effective_backend.clone()),
                entry.effective_model.clone(),
            );
            if active.contains(&key) {
                routes.insert(
                    key,
                    PaidRouteApproval {
                        profile: profile_name.into(),
                        work_id: work_id.clone(),
                        backend: entry.effective_backend.clone(),
                        backend_instance: entry.usage.backend_instance.clone(),
                        model: entry.effective_model.clone(),
                        approved: true,
                        requested: false,
                    },
                );
            }
        }
        if let Some(gate) =
            effective_human_gate_from_entries(&entries, profile_name, &profile.repo_id, &work_id)
                .filter(|gate| gate.reason_code.as_deref() == Some("policy_approval"))
        {
            if let Some(diagnostics) = gate.routing_diagnostics {
                for candidate in diagnostics.candidates.into_iter().filter(|candidate| {
                    candidate.skip_reason.as_deref() == Some("operator_approval_required")
                }) {
                    // Legacy candidates carry the logical backend as their instance
                    // projection. Only explicitly configured instances use --instance.
                    let instance = candidate.backend_instance.filter(|instance| {
                        instance != &candidate.backend
                            || routing.backend_instances.contains_key(instance)
                    });
                    let key = (
                        instance
                            .clone()
                            .unwrap_or_else(|| candidate.backend.clone()),
                        candidate.model.clone(),
                    );
                    let approved = active.contains(&key);
                    routes.entry(key).or_insert(PaidRouteApproval {
                        profile: profile_name.into(),
                        work_id: work_id.clone(),
                        backend: candidate.backend,
                        backend_instance: instance,
                        model: candidate.model,
                        approved,
                        requested: !approved,
                    });
                }
            }
        }
        result.extend(routes.into_values());
    }
    result
}

#[cfg(test)]
mod tests;
