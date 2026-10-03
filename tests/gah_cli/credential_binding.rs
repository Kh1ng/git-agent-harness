#![cfg(unix)]
use super::*;
use std::path::PathBuf;

struct BindingFixture {
    root: TempDir,
    config: PathBuf,
    path: String,
}
impl BindingFixture {
    fn new(runner: &str, script: &str) -> Self {
        let (root, config) = super::config::config_with_profile();
        let bins = root.path().join("bin");
        fs::create_dir_all(&bins).unwrap();
        write_executable(&bins.join(runner), script);
        let path = format!("{}:{}", bins.display(), std::env::var("PATH").unwrap());
        Self { root, config, path }
    }
    fn command(&self) -> IsolatedCommand<Command> {
        let mut cmd = bin();
        cmd.env("HOME", self.root.path()).env("PATH", &self.path);
        cmd
    }
    fn save(&self, id: &str, provider: &str, variable: &str, key: &str) {
        self.command()
            .args([
                "credentials",
                "save",
                "--id",
                id,
                "--provider",
                provider,
                "--kind",
                "api_key",
                "--account-label",
                id,
                "--env-var",
                variable,
            ])
            .write_stdin(key)
            .assert()
            .success();
    }
    fn add(&self, instance: &str, runner: &str, source: &str) {
        self.command()
            .args([
                "config",
                "add-backend-instance",
                "--config-path",
                self.config.to_str().unwrap(),
                "--profile",
                "test",
                "--instance",
                instance,
                "--runner-kind",
                runner,
                "--account-label",
                instance,
                "--credential-id",
                source,
            ])
            .assert()
            .success();
    }
    fn exec(&self, instance: &str) -> IsolatedCommand<Command> {
        let mut cmd = self.command();
        cmd.args([
            "config",
            "exec-backend-instance",
            "--config-path",
            self.config.to_str().unwrap(),
            "--profile",
            "test",
            "--instance",
            instance,
        ]);
        cmd
    }
    fn state(&self, instance: &str) -> PathBuf {
        self.root
            .path()
            .join(".config/gah/backend-instances")
            .join(instance)
    }
}

#[test]
fn two_named_sources_override_ambient_and_keep_account_state_separate() {
    let f = BindingFixture::new("vibe", "#!/bin/sh\nprintf '%s|%s|%s' \"$MISTRAL_API_KEY\" \"$HOME\" \"${ANTHROPIC_AUTH_TOKEN-unset}\"\n");
    f.save(
        "mistral-one",
        "mistral",
        "MISTRAL_API_KEY",
        "synthetic-first",
    );
    f.save(
        "mistral-two",
        "mistral",
        "MISTRAL_API_KEY",
        "synthetic-second",
    );
    f.add("vibe-one", "vibe", "mistral-one");
    f.add("vibe-two", "vibe", "mistral-two");
    for (instance, key) in [
        ("vibe-one", "synthetic-first"),
        ("vibe-two", "synthetic-second"),
    ] {
        f.exec(instance)
            .env("MISTRAL_API_KEY", "synthetic-ambient")
            .env("ANTHROPIC_AUTH_TOKEN", "synthetic-competing")
            .assert()
            .success()
            .stdout(format!("{key}|{}|unset", f.state(instance).display()));
    }
}

#[test]
fn missing_selected_source_never_launches_or_falls_back() {
    let f = BindingFixture::new("vibe", "#!/bin/sh\nprintf launched\n");
    f.save("source", "mistral", "MISTRAL_API_KEY", "synthetic-first");
    f.add("vibe-one", "vibe", "source");
    fs::remove_file(f.root.path().join(".config/gah/credentials/source.json")).unwrap();
    f.exec("vibe-one")
        .env("MISTRAL_API_KEY", "synthetic-ambient")
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains("named credential unavailable"));
}

#[test]
fn binding_rejects_wrong_provider_and_usage_only_admin_key() {
    let f = BindingFixture::new("vibe", "#!/bin/sh\nprintf launched\n");
    f.save("wrong", "anthropic", "ANTHROPIC_API_KEY", "synthetic-wrong");
    f.save(
        "admin",
        "mistral",
        "MISTRAL_ADMIN_API_KEY",
        "synthetic-admin",
    );
    for source in ["wrong", "admin"] {
        f.command()
            .args([
                "config",
                "add-backend-instance",
                "--config-path",
                f.config.to_str().unwrap(),
                "--profile",
                "test",
                "--instance",
                source,
                "--runner-kind",
                "vibe",
                "--account-label",
                source,
                "--credential-id",
                source,
            ])
            .assert()
            .failure();
    }
}

#[test]
fn named_key_obeys_existing_paid_external_scope_approval() {
    let f = BindingFixture::new("vibe", "#!/bin/sh\nprintf launched\n");
    f.save("source", "mistral", "MISTRAL_API_KEY", "synthetic-first");
    f.add("vibe-one", "vibe", "source");
    let mut config: toml::Value = toml::from_str(&fs::read_to_string(&f.config).unwrap()).unwrap();
    config["profiles"]["test"]["external_credential_scopes"] =
        toml::Value::try_from(serde_json::json!({"paid": {"env_vars": ["MISTRAL_API_KEY"]}}))
            .unwrap();
    fs::write(&f.config, toml::to_string(&config).unwrap()).unwrap();
    f.exec("vibe-one")
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains(
            "work-scoped external API approval",
        ));
}

#[test]
fn codex_explicit_provider_configuration_contains_env_reference_not_key() {
    let f = BindingFixture::new("codex", "#!/bin/sh\nprintf '%s|%s|%s|%s' \"$CODEX_CONFIG\" \"$MODEL_PROVIDER\" \"${CODEX_API_KEY-unset}\" \"$*\"\n");
    f.save(
        "openai-one",
        "openai",
        "MY_OPENAI_API_KEY",
        "synthetic-openai",
    );
    f.add("codex-one", "codex", "openai-one");
    let output = f
        .exec("codex-one")
        .env("CODEX_API_KEY", "synthetic-other")
        .args(["--", "exec"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("MY_OPENAI_API_KEY"));
    assert!(text.contains("requires_openai_auth=false"));
    assert!(text.contains("|gah_selected_openai|unset|exec"));
    assert!(!text.contains("synthetic-"));
}

#[test]
fn claude_project_auth_override_fails_before_launch() {
    let f = BindingFixture::new("claude", "#!/bin/sh\nprintf launched\n");
    f.save(
        "anthropic-one",
        "anthropic",
        "ANTHROPIC_API_KEY",
        "synthetic-anthropic",
    );
    f.add("claude-one", "claude", "anthropic-one");
    fs::create_dir_all(f.root.path().join(".claude")).unwrap();
    fs::write(
        f.root.path().join(".claude/settings.local.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"synthetic-competing"}}"#,
    )
    .unwrap();
    f.exec("claude-one")
        .current_dir(f.root.path())
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains("settings override"));
}

#[test]
fn opencode_provider_namespace_selects_billing_account_and_replaces_literal_key() {
    let f = BindingFixture::new(
        "opencode",
        "#!/bin/sh\nprintf '%s' \"$OPENCODE_CONFIG_CONTENT\"\n",
    );
    f.save("nous-one", "nous", "NOUS_API_KEY", "synthetic-nous");
    f.add("opencode-one", "opencode", "nous-one");
    let output = f.exec("opencode-one").args(["--model", "nous-portal/openai/gpt-5.6-luna"]).env("OPENCODE_CONFIG_CONTENT", r#"{"provider":{"nous-portal":{"options":{"apiKey":"synthetic-old","baseURL":"https://example.invalid"}},"mistral":{"options":{"other":true}}}}"#).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        config["provider"]["nous-portal"]["options"]["apiKey"],
        "{env:NOUS_API_KEY}"
    );
    assert_eq!(config["provider"]["mistral"]["options"]["other"], true);
    f.exec("opencode-one")
        .args(["--model", "openai/gpt-5.6-luna"])
        .assert()
        .failure()
        .stdout("");
}

#[test]
fn hermes_provisions_env_reference_and_rejects_native_pool_override() {
    let f = BindingFixture::new(
        "hermes",
        "#!/bin/sh\nprintf '%s|%s' \"$NOUS_API_KEY\" \"$HERMES_HOME\"\n",
    );
    f.save("nous-one", "nous", "NOUS_API_KEY", "synthetic-nous");
    f.add("hermes-one", "hermes", "nous-one");
    let home = f.state("hermes-one").join(".hermes");
    assert_eq!(
        fs::read_to_string(home.join(".env")).unwrap(),
        "NOUS_API_KEY=${NOUS_API_KEY}\n"
    );
    assert!(!fs::read_to_string(home.join("config.yaml"))
        .unwrap()
        .contains("synthetic-nous"));
    f.exec("hermes-one")
        .args(["--model", "openai/gpt-5.6-luna"])
        .assert()
        .success()
        .stdout(format!("synthetic-nous|{}", home.display()));
    fs::write(home.join("auth.json"), r#"{"credential_pool":{}}"#).unwrap();
    f.exec("hermes-one")
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains("without native credential pools"));
}

#[test]
fn openhands_selected_source_overrides_legacy_key_and_endpoint() {
    let f = BindingFixture::new(
        "openhands",
        "#!/bin/sh\nprintf '%s|%s|%s' \"$LLM_API_KEY\" \"$LLM_BASE_URL\" \"$LLM_MODEL\"\n",
    );
    f.save("nous-one", "nous", "NOUS_API_KEY", "synthetic-nous");
    f.add("openhands-one", "openhands", "nous-one");
    f.exec("openhands-one")
        .args(["--model", "openai/gpt-5.6-luna"])
        .env("LLM_API_KEY", "synthetic-old")
        .env("LLM_BASE_URL", "https://example.invalid")
        .env("LLM_MODEL", "wrong-model")
        .assert()
        .success()
        .stdout("synthetic-nous|https://inference-api.nousresearch.com/v1|openai/gpt-5.6-luna");
}

#[test]
fn custom_key_variable_does_not_bypass_standard_provider_scope() {
    let f = BindingFixture::new("codex", "#!/bin/sh\nprintf launched\n");
    f.save("source", "openai", "MY_OPENAI_API_KEY", "synthetic-first");
    f.add("codex-one", "codex", "source");
    let mut config: toml::Value = toml::from_str(&fs::read_to_string(&f.config).unwrap()).unwrap();
    config["profiles"]["test"]["external_credential_scopes"] =
        toml::Value::try_from(serde_json::json!({"paid": {"env_vars": ["OPENAI_API_KEY"]}}))
            .unwrap();
    fs::write(&f.config, toml::to_string(&config).unwrap()).unwrap();
    f.exec("codex-one")
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains(
            "work-scoped external API approval",
        ));
}

#[test]
fn acp_wrapper_preserves_stdio_and_only_selected_child_auth() {
    let f = BindingFixture::new("codex", "#!/bin/sh\nexit 0\n");
    f.save("source", "openai", "OPENAI_API_KEY", "synthetic-first");
    f.add("codex-one", "codex", "source");
    let bridge = f.root.path().join("bridge.js");
    fs::write(&bridge, "let text='';process.stdin.on('data',c=>text+=c);process.stdin.on('end',()=>process.stdout.write(JSON.stringify({text,key:process.env.OPENAI_API_KEY,other:process.env.CODEX_API_KEY||null,path:process.env.CODEX_PATH,home:process.env.HOME}))); ").unwrap();
    let output = f
        .exec("codex-one")
        .args(["--acp-bridge", bridge.to_str().unwrap(), "--"])
        .env("CODEX_API_KEY", "synthetic-competing")
        .write_stdin("from-chat")
        .output()
        .unwrap();
    assert!(output.status.success());
    let message: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(message["text"], "from-chat");
    assert_eq!(message["key"], "synthetic-first");
    assert_eq!(message["other"], Value::Null);
    assert_eq!(message["home"], f.state("codex-one").to_str().unwrap());
    assert_eq!(
        message["path"],
        f.root.path().join("bin/codex").to_str().unwrap()
    );
}
