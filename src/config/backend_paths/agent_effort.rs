use crate::config::Profile;
use anyhow::{bail, Result};
use std::collections::HashMap;

// Store effort in the existing native CLI arguments so implementation and
// review launches consume the same setting, without another dispatch override.
impl Profile {
    pub fn set_agent_effort(&mut self, backend: &str, effort: &str) -> Result<()> {
        if !matches!(
            effort,
            "default" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
        ) {
            bail!("unknown reasoning effort '{effort}'");
        }
        let args = match backend {
            "codex" => &mut self.codex_args,
            "claude" if effort != "ultra" => &mut self.claude_args,
            _ => bail!("reasoning control is not supported for '{backend}'"),
        };
        let mut kept = Vec::new();
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            if backend == "claude" && arg == "--effort" {
                index += 2;
                continue;
            }
            if backend == "claude" && arg.starts_with("--effort=") {
                index += 1;
                continue;
            }
            if backend == "codex" {
                if matches!(arg.as_str(), "-c" | "--config")
                    && args
                        .get(index + 1)
                        .is_some_and(|value| value.starts_with("model_reasoning_effort="))
                {
                    index += 2;
                    continue;
                }
                if arg.starts_with("--config=model_reasoning_effort=")
                    || arg.starts_with("-c=model_reasoning_effort=")
                {
                    index += 1;
                    continue;
                }
            }
            kept.push(arg.clone());
            index += 1;
        }
        if effort != "default" {
            if backend == "codex" {
                kept.extend(["-c".into(), format!("model_reasoning_effort=\"{effort}\"")]);
            } else {
                kept.extend(["--effort".into(), effort.into()]);
            }
        }
        *args = kept;
        Ok(())
    }

    pub fn agent_efforts(&self) -> HashMap<String, String> {
        let mut efforts = HashMap::new();
        for (backend, args) in [("codex", &self.codex_args), ("claude", &self.claude_args)] {
            for (index, arg) in args.iter().enumerate() {
                let value = if backend == "claude" {
                    arg.strip_prefix("--effort=").or_else(|| {
                        (arg == "--effort")
                            .then(|| args.get(index + 1).map(String::as_str))
                            .flatten()
                    })
                } else {
                    arg.strip_prefix("model_reasoning_effort=")
                        .or_else(|| arg.strip_prefix("--config=model_reasoning_effort="))
                        .or_else(|| arg.strip_prefix("-c=model_reasoning_effort="))
                };
                if let Some(value) = value {
                    efforts.insert(backend.into(), value.trim_matches(['\"', '\'']).into());
                }
            }
        }
        efforts
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn effort_preserves_other_flags_and_replaces_or_clears_only_effort() {
        let mut profile = crate::config::tests::test_profile_for_notifications();
        profile.codex_args = vec![
            "--sandbox".into(),
            "workspace-write".into(),
            "-c".into(),
            "model_reasoning_effort=\"low\"".into(),
            "-c".into(),
            "foo=true".into(),
        ];
        profile.set_agent_effort("codex", "high").unwrap();
        assert_eq!(profile.agent_efforts()["codex"], "high");
        assert!(profile.codex_args.contains(&"foo=true".into()));
        assert!(!profile.codex_args.iter().any(|arg| arg.contains("low")));
        profile.set_agent_effort("codex", "default").unwrap();
        assert!(!profile.agent_efforts().contains_key("codex"));
        profile.claude_args = vec!["--effort=low".into(), "--verbose".into()];
        profile.set_agent_effort("claude", "max").unwrap();
        assert_eq!(profile.claude_args, ["--verbose", "--effort", "max"]);
        assert!(profile.set_agent_effort("vibe", "high").is_err());
    }
}
