use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::OpenOptions;
use std::path::Path;

fn open(path: &Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating ledger directory {}", parent.display()))?;
    }
    let lock_path = path.with_extension("lock");
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening ledger lock {}", lock_path.display()))
}

fn lock_path(path: &Path, shared: bool) -> Result<std::fs::File> {
    let file = open(path)?;
    if shared {
        FileExt::lock_shared(&file)
            .with_context(|| format!("locking ledger {} for reading", path.display()))?;
    } else {
        file.lock_exclusive()
            .with_context(|| format!("locking ledger {}", path.display()))?;
    }
    Ok(file)
}

pub(super) fn exclusive(path: &Path) -> Result<std::fs::File> {
    lock_path(path, false)
}

pub(super) fn shared(path: &Path) -> Result<std::fs::File> {
    lock_path(path, true)
}

pub(super) fn mirror_exclusive(path: &Path) -> Result<std::fs::File> {
    let mirror_path = path.with_extension("sqlite-sync.db");
    lock_path(&mirror_path, false)
}
