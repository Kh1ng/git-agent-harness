use crate::config;
use anyhow::{Context, Result};
use std::fs;

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
    let data_root = config::default_data_root();
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
        // Issue #1366: appending a profile to an existing config must not
        // leave `defaults.worktree_base` missing or empty on disk -- the WSL
        // installer and `gah config set` can write defaults first, and an
        // empty value makes every later dispatch plan its worktree at the
        // filesystem root. Fill the init default into the file, without
        // rewriting unrelated lines or comments.
        let patched = ensure_worktree_base_default(&existing, &worktree_base);
        format!("{}\n\n{}", patched.trim_end(), profile_block)
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
    match crate::provider_kind::ProviderKind::parse(provider) {
        Ok(crate::provider_kind::ProviderKind::Gitlab) => {
            println!("Set a token in GITLAB_PAT or GITLAB_PAT2 before dispatch.")
        }
        Ok(crate::provider_kind::ProviderKind::Github) => {
            println!("Set a token in GITHUB_TOKEN or GH_TOKEN before dispatch.")
        }
        Err(_) => println!("Set provider credentials in the environment before dispatch."),
    }
}

/// The `defaults.worktree_base` value TOML actually sees in `text`: `None`
/// when the text does not parse, an empty string when the key is absent.
fn parsed_worktree_base(text: &str) -> Option<String> {
    let table = text.parse::<toml::Table>().ok()?;
    Some(
        table
            .get("defaults")
            .and_then(|defaults| defaults.get("worktree_base"))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string(),
    )
}

/// True for a `[defaults]` table header line, tolerating inner whitespace and
/// a trailing comment (`[ defaults ] # shared settings`).
fn is_defaults_header(line: &str) -> bool {
    let header = line.split('#').next().unwrap_or_default().trim();
    header
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .is_some_and(|name| name.trim() == "defaults")
}

/// Fill `default_base` into the existing config's `[defaults]` section when
/// `worktree_base` is missing or empty. The edit is line-based so unrelated
/// keys, comments, and formatting survive untouched, but TOML decides both
/// whether a value is already set and whether the edit is kept: a patch that
/// does not parse back to `default_base` is discarded, so init can never
/// turn a loadable config into an unparseable one.
fn ensure_worktree_base_default(existing: &str, default_base: &str) -> String {
    match parsed_worktree_base(existing) {
        Some(value) if value.trim().is_empty() => {}
        // Already configured, or not parseable to begin with: leave it alone.
        _ => return existing.to_string(),
    }
    let patched = patch_worktree_base_line(existing, default_base);
    if parsed_worktree_base(&patched).as_deref() == Some(default_base) {
        patched
    } else {
        // Dispatch resolves an empty value to the default at the point of
        // use, so keeping the original text is safe.
        existing.to_string()
    }
}

fn patch_worktree_base_line(existing: &str, default_base: &str) -> String {
    let key_line = format!("worktree_base = \"{}\"", default_base);
    let lines: Vec<&str> = existing.lines().collect();
    let defaults_header = lines.iter().position(|line| is_defaults_header(line));
    let Some(header) = defaults_header else {
        // No `[defaults]` table at all: append one; TOML allows tables in any
        // order, so a trailing `[defaults]` stays valid.
        return format!("{}\n\n[defaults]\n{}", existing.trim_end(), key_line);
    };
    let body_end = lines
        .iter()
        .enumerate()
        .skip(header + 1)
        .find(|(_, line)| line.trim_start().starts_with('['))
        .map(|(index, _)| index)
        .unwrap_or(lines.len());
    let existing_key = lines[header + 1..body_end].iter().position(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix("worktree_base")
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    });
    let mut out: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
    match existing_key {
        // An empty `worktree_base = ""` (or whitespace) inside the section:
        // replace it in place.
        Some(offset) => {
            out[header + 1 + offset] = key_line;
        }
        // No key in the section: insert one directly under the header.
        None => {
            out.insert(header + 1, key_line);
        }
    }
    let mut text = out.join("\n");
    if existing.ends_with('\n') && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{ensure_worktree_base_default, parsed_worktree_base, render_profile, InitArgs};

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

    const DEFAULT_BASE: &str = "/home/tester/.local/share/gah/worktrees";

    #[test]
    fn append_fills_worktree_base_into_defaults_section() {
        // Issue #1366: a config written without worktree_base (e.g. by the
        // WSL installer) must gain the init default when a profile is added.
        let existing = "[defaults]\nartifact_root = \"/srv/gah\"\n\n[profiles]\n";
        let patched = ensure_worktree_base_default(existing, DEFAULT_BASE);
        assert!(patched.contains(&format!("worktree_base = \"{DEFAULT_BASE}\"")));
        assert!(patched.contains("artifact_root = \"/srv/gah\""));
        // The key lands inside [defaults], before the next table.
        assert!(
            patched
                .lines()
                .position(|line| line.trim() == "[defaults]")
                .unwrap()
                < patched
                    .lines()
                    .position(|line| line.contains("worktree_base"))
                    .unwrap()
        );
        // The result must stay parseable and carry the value.
        let cfg: crate::config::GahConfig = toml::from_str(&patched).unwrap();
        assert_eq!(cfg.defaults.worktree_base, DEFAULT_BASE);
    }

    #[test]
    fn append_replaces_empty_worktree_base_in_place() {
        let existing = "[defaults]\nworktree_base = \"\"\nartifact_root = \"/srv/gah\"\n";
        let patched = ensure_worktree_base_default(existing, DEFAULT_BASE);
        assert_eq!(
            patched,
            format!(
                "[defaults]\nworktree_base = \"{DEFAULT_BASE}\"\nartifact_root = \"/srv/gah\"\n"
            )
        );
        let cfg: crate::config::GahConfig = toml::from_str(&patched).unwrap();
        assert_eq!(cfg.defaults.worktree_base, DEFAULT_BASE);
    }

    #[test]
    fn append_preserves_configured_worktree_base_and_comments() {
        let existing = "# operator comment\n[defaults]\n# keep this\nworktree_base = \"/custom/base\"\n[profiles]\n";
        let patched = ensure_worktree_base_default(existing, DEFAULT_BASE);
        assert_eq!(patched, existing);
    }

    #[test]
    fn append_adds_defaults_table_when_missing() {
        let existing = "[profiles.existing]\ndisplay_name = \"Existing\"\nrepo_id = \"existing\"\nrepo = \"a/b\"\nprovider = \"github\"\nlocal_path = \"/tmp/repo\"\nartifact_root = \"/srv/gah/existing\"\ndefault_target_branch = \"main\"\n";
        let patched = ensure_worktree_base_default(existing, DEFAULT_BASE);
        let cfg: crate::config::GahConfig = toml::from_str(&patched).unwrap();
        assert_eq!(cfg.defaults.worktree_base, DEFAULT_BASE);
        assert!(patched.contains("[profiles.existing]"));
    }

    #[test]
    fn append_ignores_worktree_base_keys_outside_defaults() {
        // A root-level key is not the [defaults] value; the section still
        // gets the default so dispatch never plans at the filesystem root.
        let existing =
            "worktree_base = \"/root-level-trap\"\n[defaults]\nartifact_root = \"/srv\"\n";
        let patched = ensure_worktree_base_default(existing, DEFAULT_BASE);
        let cfg: crate::config::GahConfig = toml::from_str(&patched).unwrap();
        assert_eq!(cfg.defaults.worktree_base, DEFAULT_BASE);
    }

    #[test]
    fn append_recognises_commented_and_spaced_defaults_headers() {
        // A valid header that is not literally `[defaults]` must not gain a
        // second `[defaults]` table: TOML rejects a redefined table and every
        // later `gah` command would fail at config load.
        for header in ["[defaults] # shared settings", "[ defaults ]"] {
            let set = format!("{header}\nworktree_base = \"/custom/base\"\n\n[profiles]\n");
            assert_eq!(ensure_worktree_base_default(&set, DEFAULT_BASE), set);

            let unset = format!("{header}\nartifact_root = \"/srv/gah\"\n\n[profiles]\n");
            let patched = ensure_worktree_base_default(&unset, DEFAULT_BASE);
            assert_eq!(patched.matches("defaults").count(), 1, "{patched}");
            assert!(patched.contains(header));
            let cfg: crate::config::GahConfig = toml::from_str(&patched).unwrap();
            assert_eq!(cfg.defaults.worktree_base, DEFAULT_BASE);
        }
    }

    #[test]
    fn append_fills_values_toml_reads_as_empty() {
        for line in ["worktree_base = \"\" # unset", "worktree_base = ''"] {
            let existing = format!("[defaults]\n{line}\n");
            let patched = ensure_worktree_base_default(&existing, DEFAULT_BASE);
            assert_eq!(
                parsed_worktree_base(&patched).as_deref(),
                Some(DEFAULT_BASE)
            );
        }
    }

    #[test]
    fn append_keeps_original_when_patch_would_not_parse() {
        // Shapes the line-based edit cannot place a key into: a multi-line
        // array hiding an empty key from the section scan, and `defaults`
        // written as an inline or dotted table. The patch would duplicate a
        // key or table, so the original (still loadable) text is kept.
        for existing in [
            "[defaults]\nextra = [\n[1],\n]\nworktree_base = \"\"\n",
            "defaults = { artifact_root = \"/srv/gah\" }\n",
            "defaults.artifact_root = \"/srv/gah\"\n",
        ] {
            assert!(parsed_worktree_base(existing).is_some(), "{existing}");
            assert_eq!(
                ensure_worktree_base_default(existing, DEFAULT_BASE),
                existing
            );
        }
    }

    #[test]
    fn append_leaves_unparseable_config_untouched() {
        let existing = "[defaults\nartifact_root = \n";
        assert_eq!(
            ensure_worktree_base_default(existing, DEFAULT_BASE),
            existing
        );
    }
}
