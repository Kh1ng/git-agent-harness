//! Finish an update with the CLI it just installed (#1408).

use super::{HostRole, UpdateArgs};
use anyhow::{bail, Context, Result};
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

/// Set on the re-run by the newly installed binary, which skips the install
/// it just inherited and never re-runs itself again.
pub(super) const ENV: &str = "GAH_UPDATE_REEXEC";

/// Run the same update with `binary` and return its result. Everything after
/// the install (unit rendering, installers) is code the pull may have
/// changed; an old binary once installed an unrendered gah-server.service
/// and the restart failed.
pub(super) fn continue_with(binary: &Path, args: &UpdateArgs, repo: &Path) -> Result<()> {
    println!("Continuing the update with the new CLI");
    let status = Command::new(binary)
        .args(argv(args, repo))
        .env(ENV, "1")
        .status()
        .with_context(|| format!("starting {}", binary.display()))?;
    if !status.success() {
        bail!("{} update exited with {status}", binary.display());
    }
    Ok(())
}

fn argv(args: &UpdateArgs, repo: &Path) -> Vec<OsString> {
    let role = match args.role {
        HostRole::Central => "central",
        HostRole::Standalone => "standalone",
        HostRole::Worker => "worker",
    };
    let mut out: Vec<OsString> = vec![
        "update".into(),
        "--repo".into(),
        repo.into(),
        "--role".into(),
        role.into(),
    ];
    if args.restart_server {
        out.push("--restart-server".into());
    }
    out.extend([
        "--server-service".into(),
        args.server_service.clone().into(),
    ]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_the_same_update_request() {
        let args = UpdateArgs {
            repo: None,
            role: HostRole::Central,
            restart_server: true,
            server_service: "gah-server.service".into(),
        };
        let argv: Vec<String> = argv(&args, Path::new("/srv/gah"))
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect();
        assert_eq!(
            argv,
            [
                "update",
                "--repo",
                "/srv/gah",
                "--role",
                "central",
                "--restart-server",
                "--server-service",
                "gah-server.service"
            ]
        );
    }
}
