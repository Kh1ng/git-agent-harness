//! Apply selected instance state and inference credentials at the runner boundary.
use crate::config::Profile;
use crate::execution_identity::ExecutionIdentity;
use anyhow::Result;

pub(in crate::dispatch) fn apply_execution_identity_env(
    profile: &Profile,
    identity: &crate::execution_identity::ExecutionIdentity,
    env_vars: &mut Vec<(String, String)>,
) -> Result<()> {
    if identity.state_root.is_some() {
        identity.apply_instance_state_env(env_vars);
    } else {
        super::apply_backend_instance_env(profile, &identity.logical_backend, env_vars);
    }
    identity.apply_credential_env(env_vars)
}

pub(super) fn selected_llm(
    identity: &ExecutionIdentity,
    llm: &crate::runner::LlmConfig,
    env_vars: &[(String, String)],
) -> Result<crate::runner::LlmConfig> {
    let mut selected_llm = crate::runner::LlmConfig {
        base_url: llm.base_url.clone(),
        api_key: llm.api_key.clone(),
        model: llm.model.clone(),
    };
    if identity.credential_id.is_some() && identity.runner_kind == "openhands" {
        selected_llm.base_url = env_vars
            .iter()
            .rev()
            .find(|(name, _)| name == "LLM_BASE_URL")
            .ok_or_else(|| anyhow::anyhow!("selected credential has no OpenHands endpoint"))?
            .1
            .clone();
        selected_llm.api_key = env_vars
            .iter()
            .rev()
            .find(|(name, _)| name == "LLM_API_KEY")
            .ok_or_else(|| anyhow::anyhow!("selected credential has no OpenHands execution key"))?
            .1
            .clone();
    }
    Ok(selected_llm)
}
