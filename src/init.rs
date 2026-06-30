use crate::config;
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

pub struct InitArgs {
    pub profile: String,
    pub display_name: String,
    pub provider: String,
    pub repo: String,
    pub local_path: String,
    pub default_target_branch: String,
    pub provider_api_base: Option<String>,
    pub provider_project_id: Option<String>,
    pub artifact_root: Option<String>,
    pub worktree_base: Option<String>,
    pub oh_profile: Option<String>,
    pub config_path: Option<String>,
    pub print: bool,
}

pub fn run(args: InitArgs) -> Result<()> {
    let config_path = config::resolve_config_path(args.config_path.as_deref());
    let data_root = default_data_root();
    let artifact_root = args
        .artifact_root
        .clone()
        .unwrap_or_else(|| data_root.join("artifacts").display().to_string());
    let worktree_base = args
        .worktree_base
        .clone()
        .unwrap_or_else(|| data_root.join("worktrees").display().to_string());

    let defaults_block = format!(
        "[defaults]\nartifact_root = \"{}\"\nworktree_base = \"{}\"\nllm_base_url = \"\"\nllm_model_local = \"\"\nllm_model_cloud = \"\"\n",
        artifact_root, worktree_base
    );
    let profile_block = render_profile(&args, &artifact_root);

    if args.print {
        if !config_path.exists() {
            println!("{}", defaults_block);
        }
        println!("{}", profile_block);
        print_secret_hint(&args.provider);
        return Ok(());
    }

    let existing = if config_path.exists() {
        fs::read_to_string(&config_path)
            .with_context(|| format!("reading {}", config_path.display()))?
    } else {
        String::new()
    };
    if existing.contains(&format!("[profiles.{}]", args.profile)) {
        anyhow::bail!(
            "profile '{}' already exists in {}",
            args.profile,
            config_path.display()
        );
    }

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let out = if existing.trim().is_empty() {
        format!("{}\n{}", defaults_block, profile_block)
    } else {
        format!("{}\n\n{}", existing.trim_end(), profile_block)
    };
    fs::write(&config_path, out).with_context(|| format!("writing {}", config_path.display()))?;

    println!("Wrote {}", config_path.display());
    print_secret_hint(&args.provider);
    Ok(())
}

fn render_profile(args: &InitArgs, artifact_root: &str) -> String {
    let mut out = format!(
        "[profiles.{}]\n\
display_name = \"{}\"\n\
repo_id = \"{}\"\n\
provider = \"{}\"\n\
repo = \"{}\"\n\
local_path = \"{}\"\n\
artifact_root = \"{}/{}\"\n\
default_target_branch = \"{}\"\n",
        args.profile,
        args.display_name,
        args.profile,
        args.provider,
        args.repo,
        args.local_path,
        artifact_root.trim_end_matches('/'),
        args.profile,
        args.default_target_branch,
    );
    if let Some(api) = &args.provider_api_base {
        out.push_str(&format!("provider_api_base = \"{}\"\n", api));
    }
    if let Some(project_id) = &args.provider_project_id {
        out.push_str(&format!("provider_project_id = \"{}\"\n", project_id));
    }
    if let Some(oh_profile) = &args.oh_profile {
        out.push_str(&format!("oh_profile = \"{}\"\n", oh_profile));
    }
    out.push_str("validation_commands = []\n");
    out
}

fn print_secret_hint(provider: &str) {
    match provider {
        "gitlab" => println!("Set a token in GITLAB_PAT or GITLAB_PAT2 before dispatch."),
        "github" => println!("Set a token in GITHUB_TOKEN or GH_TOKEN before dispatch."),
        _ => println!("Set provider credentials in the environment before dispatch."),
    }
}

fn default_data_root() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    PathBuf::from(home).join(".local/share/gah")
}

#[cfg(test)]
mod tests {
    use super::{render_profile, InitArgs};

    #[test]
    fn render_profile_includes_optional_gitlab_fields() {
        let args = InitArgs {
            profile: "sample".into(),
            display_name: "Sample".into(),
            provider: "gitlab".into(),
            repo: "group/repo".into(),
            local_path: "/tmp/repo".into(),
            default_target_branch: "main".into(),
            provider_api_base: Some("https://gitlab.example.com/api/v4".into()),
            provider_project_id: Some("42".into()),
            artifact_root: None,
            worktree_base: None,
            oh_profile: Some("cloud".into()),
            config_path: None,
            print: true,
        };
        let block = render_profile(&args, "/tmp/artifacts");
        assert!(block.contains("provider_api_base = \"https://gitlab.example.com/api/v4\""));
        assert!(block.contains("provider_project_id = \"42\""));
        assert!(block.contains("oh_profile = \"cloud\""));
    }
}
