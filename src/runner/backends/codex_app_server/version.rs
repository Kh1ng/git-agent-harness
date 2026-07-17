//! Records the two facts GAH must know about the installed Codex binary
//! before trusting its app-server protocol: which binary version answered
//! the handshake, and a digest of the JSON Schema it currently generates.
//! Both travel together as [`CodexAppServerInfo`] so a later capability
//! mismatch can be attributed to a specific, comparable pair rather than
//! silently assumed away.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct CodexAppServerInfo {
    pub(crate) binary_version: String,
    pub(crate) schema_digest: String,
}

/// The `manifest.json` written by `scripts/generate-codex-schemas.js` for
/// one versioned generated package: which binary produced it, its schema
/// digest, and the methods that only appear with `--experimental`. This
/// is the authoritative, per-version experimental-method list that
/// [`super::CodexAppServerOptions`] uses to gate calls client-side.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodexSchemaManifest {
    pub(crate) codex_binary_version: String,
    pub(crate) schema_digest: String,
    pub(crate) experimental_methods: Vec<String>,
}

/// Loads a generated-package manifest. Missing/unreadable/malformed
/// manifests are a hard error here (fail visibly) -- callers that want to
/// tolerate an absent manifest (e.g. it hasn't been generated yet in this
/// checkout) should catch that at the call site rather than have this
/// function paper over it with an empty default.
pub(crate) fn load_schema_manifest(manifest_path: &Path) -> Result<CodexSchemaManifest> {
    let text = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("parsing {} as a schema manifest", manifest_path.display()))
}

/// Compares the manifest's pinned binary version against what's actually
/// installed. A mismatch means the experimental-method list (and
/// recorded schema digest) may no longer describe the running binary --
/// this must fail visibly rather than silently trust a stale manifest.
pub(crate) fn check_version_drift(
    manifest: &CodexSchemaManifest,
    actual_binary_version: &str,
) -> Result<()> {
    if manifest.codex_binary_version != actual_binary_version {
        anyhow::bail!(
            "codex app-server version drift: generated schema package pins '{}' \
             but the installed binary reports '{}'; regenerate with \
             `npm run generate:codex-schemas`",
            manifest.codex_binary_version,
            actual_binary_version
        );
    }
    Ok(())
}

/// Runs `<executable> --version` and extracts the version token, e.g.
/// `codex-cli 0.144.5` -> `0.144.5`.
pub(crate) fn detect_codex_version(executable: &Path) -> Result<String> {
    let output = Command::new(executable)
        .arg("--version")
        .output()
        .with_context(|| format!("running {} --version", executable.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "{} --version exited with {:?}",
            executable.display(),
            output.status.code()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    parse_codex_version(&text)
        .with_context(|| format!("unrecognized `codex --version` output: {text:?}"))
}

fn parse_codex_version(text: &str) -> Option<String> {
    let token = text.trim().rsplit(' ').next()?;
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// A self-cleaning temp directory. `tempfile` is a dev-only dependency
/// (see Cargo.toml), so this schema-digest path -- which runs in normal
/// (non-test) builds -- uses a small hand-rolled equivalent instead of
/// promoting `tempfile` to a runtime dependency for one call site.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn create(label: &str) -> Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("gah-{label}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating scratch dir {}", dir.display()))?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs `<executable> app-server generate-json-schema` into a scratch
/// directory and hashes every generated `.json` file (name and contents,
/// sorted by filename for determinism) into a single digest. This is the
/// same generator the versioned schema-generation tooling
/// (`scripts/generate-codex-schemas.sh`) uses, so the recorded digest is
/// directly comparable to what that tooling produced for a given binary.
pub(crate) fn compute_schema_digest(executable: &Path) -> Result<String> {
    compute_schema_digest_with_flags(executable, &[])
}

fn compute_schema_digest_with_flags(executable: &Path, extra_flags: &[&str]) -> Result<String> {
    let scratch = ScratchDir::create("codex-schema")?;
    let status = Command::new(executable)
        .args(["app-server", "generate-json-schema", "--out"])
        .arg(scratch.path())
        .args(extra_flags)
        .status()
        .with_context(|| {
            format!(
                "running {} app-server generate-json-schema",
                executable.display()
            )
        })?;
    if !status.success() {
        anyhow::bail!(
            "codex app-server generate-json-schema exited with {:?}",
            status.code()
        );
    }

    let mut files: Vec<PathBuf> = std::fs::read_dir(scratch.path())
        .context("reading generated schema directory")?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    if files.is_empty() {
        anyhow::bail!("codex app-server generate-json-schema produced no .json files");
    }

    let mut hasher = Sha256::new();
    for file in &files {
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        hasher.update(name.as_bytes());
        hasher.update([0u8]);
        let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
        hasher.update(&bytes);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Records both facts for a given binary. Fails visibly (rather than
/// recording a placeholder) if either step fails -- per the issue's
/// requirement to fail visibly on version/capability drift instead of
/// fabricating unknown data as a default.
pub(crate) fn record_codex_app_server_info(executable: &Path) -> Result<CodexAppServerInfo> {
    let binary_version = detect_codex_version(executable)?;
    let schema_digest = compute_schema_digest(executable)?;
    Ok(CodexAppServerInfo {
        binary_version,
        schema_digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::backends::test_util::*;

    #[test]
    fn parses_typical_version_output() {
        assert_eq!(
            parse_codex_version("codex-cli 0.144.5\n"),
            Some("0.144.5".to_string())
        );
    }

    #[test]
    fn parses_version_output_without_trailing_newline() {
        assert_eq!(
            parse_codex_version("codex-cli 1.2.3"),
            Some("1.2.3".to_string())
        );
    }

    #[test]
    fn rejects_empty_version_output() {
        assert_eq!(parse_codex_version(""), None);
    }

    #[test]
    fn loads_a_well_formed_manifest() {
        let tmp = tempfile::TempDir::new().unwrap();
        let manifest_path = tmp.path().join("manifest.json");
        std::fs::write(
            &manifest_path,
            r#"{
                "codexBinaryVersion": "0.144.5",
                "schemaDigest": "sha256:abc",
                "experimentalMethods": ["mock/experimentalMethod", "process/spawn"],
                "generatedAt": "2026-01-01T00:00:00Z"
            }"#,
        )
        .unwrap();

        let manifest = load_schema_manifest(&manifest_path).unwrap();
        assert_eq!(manifest.codex_binary_version, "0.144.5");
        assert_eq!(manifest.schema_digest, "sha256:abc");
        assert_eq!(
            manifest.experimental_methods,
            vec![
                "mock/experimentalMethod".to_string(),
                "process/spawn".to_string()
            ]
        );
    }

    #[test]
    fn missing_manifest_fails_visibly_instead_of_defaulting() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = load_schema_manifest(&tmp.path().join("does-not-exist.json")).unwrap_err();
        assert!(err.to_string().contains("reading"));
    }

    #[test]
    fn version_drift_is_detected() {
        let manifest = CodexSchemaManifest {
            codex_binary_version: "0.144.5".to_string(),
            schema_digest: "sha256:abc".to_string(),
            experimental_methods: vec![],
        };
        assert!(check_version_drift(&manifest, "0.144.5").is_ok());
        let err = check_version_drift(&manifest, "0.145.0").unwrap_err();
        assert!(err.to_string().contains("version drift"));
    }

    #[test]
    fn detect_codex_version_uses_the_fake_binary() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "fake-codex",
            "#!/bin/sh\necho 'codex-cli 9.9.9'\n",
        );

        let version = detect_codex_version(&f.bin_dir.join("fake-codex")).unwrap();
        assert_eq!(version, "9.9.9");
    }

    #[test]
    fn detect_codex_version_surfaces_nonzero_exit() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(&f.bin_dir, "fake-codex", "#!/bin/sh\nexit 3\n");

        let err = detect_codex_version(&f.bin_dir.join("fake-codex")).unwrap_err();
        assert!(err.to_string().contains("exited with"));
    }

    #[test]
    fn compute_schema_digest_is_deterministic_for_fixed_output() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "fake-codex",
            "#!/bin/sh\n\
             out=\"\"\n\
             prev=\"\"\n\
             for a in \"$@\"; do\n\
               if [ \"$prev\" = \"--out\" ]; then out=\"$a\"; fi\n\
               prev=\"$a\"\n\
             done\n\
             printf '{\"a\":1}' > \"$out/A.json\"\n\
             printf '{\"b\":2}' > \"$out/B.json\"\n",
        );

        let digest_a = compute_schema_digest(&f.bin_dir.join("fake-codex")).unwrap();
        let digest_b = compute_schema_digest(&f.bin_dir.join("fake-codex")).unwrap();
        assert_eq!(digest_a, digest_b);
        assert!(digest_a.starts_with("sha256:"));
    }

    #[test]
    fn compute_schema_digest_surfaces_generator_failure() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(&f.bin_dir, "fake-codex", "#!/bin/sh\nexit 1\n");

        let err = compute_schema_digest(&f.bin_dir.join("fake-codex")).unwrap_err();
        assert!(err.to_string().contains("exited with"));
    }

    #[test]
    fn compute_schema_digest_differs_when_content_changes() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let f = fixture();
        make_fake_bin(
            &f.bin_dir,
            "fake-codex-v1",
            "#!/bin/sh\n\
             out=\"\"\n\
             prev=\"\"\n\
             for a in \"$@\"; do\n\
               if [ \"$prev\" = \"--out\" ]; then out=\"$a\"; fi\n\
               prev=\"$a\"\n\
             done\n\
             printf '{\"a\":1}' > \"$out/A.json\"\n",
        );
        make_fake_bin(
            &f.bin_dir,
            "fake-codex-v2",
            "#!/bin/sh\n\
             out=\"\"\n\
             prev=\"\"\n\
             for a in \"$@\"; do\n\
               if [ \"$prev\" = \"--out\" ]; then out=\"$a\"; fi\n\
               prev=\"$a\"\n\
             done\n\
             printf '{\"a\":2}' > \"$out/A.json\"\n",
        );

        let digest_v1 = compute_schema_digest(&f.bin_dir.join("fake-codex-v1")).unwrap();
        let digest_v2 = compute_schema_digest(&f.bin_dir.join("fake-codex-v2")).unwrap();
        assert_ne!(digest_v1, digest_v2);
    }

    /// Real-binary smoke test (verification bucket: "installed-binary
    /// handshake smoke"). Skips instead of failing when `codex` isn't on
    /// PATH so the suite stays green in environments without it.
    #[test]
    fn installed_binary_version_and_digest_smoke() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let Some(codex) = which_codex() else {
            eprintln!("skipping installed_binary_version_and_digest_smoke: codex not on PATH");
            return;
        };

        let info = record_codex_app_server_info(&codex).unwrap();
        assert!(!info.binary_version.is_empty());
        assert!(info.schema_digest.starts_with("sha256:"));
    }

    fn which_codex() -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join("codex"))
            .find(|candidate| candidate.is_file())
    }
}
