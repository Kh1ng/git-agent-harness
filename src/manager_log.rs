//! Append-only observations from supervising managers; never used for routing.
use anyhow::{bail, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[value(rename_all = "snake_case")]
pub enum Phase {
    Research,
    Implement,
    Repair,
    Supervise,
    Verify,
    Merge,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[value(rename_all = "snake_case")]
pub enum Diagnosis {
    SetupEnvironment,
    Credentials,
    FailedCheck,
    MisunderstoodTask,
    AmbiguousRequirement,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub schema_version: u32,
    pub ts: String,
    pub work_id: String,
    pub phase: Phase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnosis: Option<Diagnosis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intervention: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manager_rounds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}
impl Event {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            bail!("unsupported schema_version");
        }
        if self.work_id.trim().is_empty() {
            bail!("work_id must be non-empty");
        }
        if self.tier.is_some_and(|n| !(1..=4).contains(&n)) {
            bail!("tier must be 1 to 4");
        }
        if self.attempt == Some(0) {
            bail!("attempt must be 1 or more");
        }
        let ts = OffsetDateTime::parse(&self.ts, &Rfc3339)?;
        if ts.offset() != time::UtcOffset::UTC {
            bail!("ts must be UTC");
        }
        Ok(())
    }
}
pub fn path(defaults: &crate::config::Defaults) -> PathBuf {
    defaults
        .ledger_path()
        .parent()
        .unwrap_or(Path::new("."))
        .join("manager-log.jsonl")
}
pub fn append(path: &Path, event: &Event) -> Result<Event> {
    event.validate()?;
    let mut value = serde_json::to_value(event)?;
    crate::redact::redact_json_value(&mut value);
    let stored: Event = serde_json::from_value(value)?;
    stored.validate()?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))?;
    lock.lock_exclusive()?;
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)?;
    if file.metadata()?.len() > 0 {
        file.seek(SeekFrom::End(-1))?;
        let mut last = [0];
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            file.write_all(b"\n")?;
        }
    }
    serde_json::to_writer(&mut file, &stored)?;
    file.write_all(b"\n")?;
    Ok(stored)
}
/// Whether two work ids name the same item. Uses the ledger's aliases
/// (`#12` and `TICKET-12`), and also reads a bare number as an issue number,
/// because `--work-id 12` is what a person types.
pub fn aliases(a: &str, b: &str) -> bool {
    fn ids(id: &str) -> Vec<String> {
        let id = id.trim();
        if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
            return crate::ledger::gates::work_id_aliases(&format!("#{id}"));
        }
        crate::ledger::gates::work_id_aliases(id)
    }
    let aa = ids(a);
    ids(b).iter().any(|id| aa.contains(id))
}
#[derive(Serialize)]
pub struct Events {
    pub schema_version: u32,
    pub path: PathBuf,
    pub skipped_lines: usize,
    pub events: Vec<Event>,
}
pub fn load(path: &Path, work_id: Option<&str>) -> Result<Events> {
    let mut result = Events {
        schema_version: 1,
        path: path.into(),
        skipped_lines: 0,
        events: vec![],
    };
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(e) => return Err(e.into()),
    };
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        let event = serde_json::from_slice::<Event>(&line)
            .ok()
            .filter(|e| e.validate().is_ok());
        match event {
            Some(e) if line.last() == Some(&b'\n') => {
                if work_id.is_none_or(|id| aliases(id, &e.work_id)) {
                    result.events.push(e);
                }
            }
            _ => result.skipped_lines += 1,
        }
    }
    Ok(result)
}
#[derive(Debug, Serialize)]
pub struct Item {
    pub work_id: String,
    pub events: u64,
    pub attempts: u64,
    pub manager_rounds: u64,
    pub tokens: u64,
    pub elapsed_seconds: u64,
    pub first_ts: String,
    pub last_ts: String,
    pub last_outcome: Option<String>,
    pub last_phase: Phase,
}
#[derive(Serialize)]
pub struct Summary {
    pub schema_version: u32,
    pub path: PathBuf,
    pub skipped_lines: usize,
    pub items: Vec<Item>,
}
pub fn summarize(log: &Events) -> Result<Summary> {
    let mut items: Vec<Item> = vec![];
    for e in &log.events {
        let index = items
            .iter()
            .position(|i| aliases(&i.work_id, &e.work_id))
            .unwrap_or_else(|| {
                items.push(Item {
                    work_id: e.work_id.clone(),
                    events: 0,
                    attempts: 0,
                    manager_rounds: 0,
                    tokens: 0,
                    elapsed_seconds: 0,
                    first_ts: e.ts.clone(),
                    last_ts: e.ts.clone(),
                    last_outcome: None,
                    last_phase: e.phase,
                });
                items.len() - 1
            });
        let i = &mut items[index];
        i.events += 1;
        i.attempts = i.attempts.max(e.attempt.unwrap_or(0));
        i.manager_rounds = i.manager_rounds.max(e.manager_rounds.unwrap_or(0));
        i.tokens = i
            .tokens
            .checked_add(e.tokens.unwrap_or(0))
            .ok_or_else(|| anyhow::anyhow!("tokens sum overflow"))?;
        i.elapsed_seconds = i
            .elapsed_seconds
            .checked_add(e.elapsed_seconds.unwrap_or(0))
            .ok_or_else(|| anyhow::anyhow!("elapsed_seconds sum overflow"))?;
        i.last_ts = e.ts.clone();
        i.last_phase = e.phase;
        if e.outcome.is_some() {
            i.last_outcome.clone_from(&e.outcome);
        }
    }
    Ok(Summary {
        schema_version: 1,
        path: log.path.clone(),
        skipped_lines: log.skipped_lines,
        items,
    })
}
#[cfg(test)]
mod tests;
