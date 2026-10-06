use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct Choices {
    pub role: String,
    pub agent: String,
    pub provider: String,
    pub memory: String,
    pub gateway_url: String,
}

impl Choices {
    pub fn args(&self) -> Result<Vec<String>, String> {
        if !["standalone", "central"].contains(&self.role.as_str())
            || !["claude", "codex", "opencode"].contains(&self.agent.as_str())
            || !["github", "gitlab"].contains(&self.provider.as_str())
            || !["off", "remote"].contains(&self.memory.as_str())
        {
            return Err(
                "Choose a supported role, agent, repository provider and memory mode.".into(),
            );
        }
        let mut args = vec![
            "setup".into(),
            "--role".into(),
            self.role.clone(),
            "--agent".into(),
            self.agent.clone(),
            "--provider".into(),
            self.provider.clone(),
            "--memory".into(),
            self.memory.clone(),
        ];
        if self.memory == "remote" {
            let url = url::Url::parse(&self.gateway_url)
                .map_err(|_| "Enter a memory gateway HTTP(S) URL.")?;
            if !["http", "https"].contains(&url.scheme())
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err("Use an HTTP(S) gateway URL without credentials.".into());
            }
            args.extend(["--gateway-url".into(), self.gateway_url.clone()]);
        }
        Ok(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_memory_requires_a_valid_gateway_without_url_credentials() {
        let mut choice = Choices {
            role: "central".into(),
            agent: "claude".into(),
            provider: "github".into(),
            memory: "remote".into(),
            gateway_url: String::new(),
        };
        assert!(choice.args().is_err());
        choice.gateway_url = "file:///etc/passwd".into();
        assert!(choice.args().is_err());
        choice.gateway_url = "https://memory.example.com".into();
        let args = choice.args().unwrap();
        assert_eq!(
            &args[args.len() - 2..],
            ["--gateway-url", "https://memory.example.com"]
        );
    }

    #[test]
    fn choices_are_explicit_and_reject_shell_or_secret_urls() {
        let mut choice = Choices {
            role: "standalone".into(),
            agent: "codex".into(),
            provider: "gitlab".into(),
            memory: "off".into(),
            gateway_url: String::new(),
        };
        assert_eq!(
            choice.args().unwrap(),
            [
                "setup",
                "--role",
                "standalone",
                "--agent",
                "codex",
                "--provider",
                "gitlab",
                "--memory",
                "off"
            ]
        );
        choice.role = "standalone; touch /tmp/unwanted".into();
        assert!(choice.args().is_err());
        choice.role = "standalone".into();
        choice.memory = "remote".into();
        choice.gateway_url = "https://user:secret@example.com".into();
        assert!(choice.args().is_err());
    }
}
