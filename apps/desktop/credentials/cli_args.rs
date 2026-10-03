//! Shared by the native bridge and the real GAH parser contract tests.
pub fn list() -> Vec<String> {
    ["credentials", "list", "--json"].map(str::to_owned).into()
}

pub fn save(
    id: &str,
    provider: &str,
    kind: &str,
    label: &str,
    env_var: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "credentials",
        "save",
        "--id",
        id,
        "--provider",
        provider,
        "--kind",
        kind,
        "--account-label",
        label,
        "--json",
    ]
    .map(str::to_owned)
    .into();
    if let Some(env_var) = env_var {
        args.extend(["--env-var".to_owned(), env_var.to_owned()]);
    }
    args
}

pub fn remove(id: &str) -> Vec<String> {
    ["credentials", "remove", "--id", id]
        .map(str::to_owned)
        .into()
}

pub fn refresh(id: &str) -> Vec<String> {
    ["quota", "refresh", "--credential", id]
        .map(str::to_owned)
        .into()
}

pub fn instances() -> Vec<String> {
    ["config", "show", "--json", "--full"]
        .map(str::to_owned)
        .into()
}

pub fn bind(profile: &str, instance: &str, credential_id: &str) -> Vec<String> {
    [
        "config",
        "set-backend-instance-credential",
        "--profile",
        profile,
        "--instance",
        instance,
        "--credential-id",
        credential_id,
    ]
    .map(str::to_owned)
    .into()
}
