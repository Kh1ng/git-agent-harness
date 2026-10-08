use crate::{
    cli::args::ManagerLogCommands,
    config,
    manager_log::{self, Event},
};
use anyhow::Result;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
pub fn run(command: ManagerLogCommands) -> Result<()> {
    match command {
        ManagerLogCommands::Add(args) => {
            let event = Event {
                schema_version: 1,
                ts: OffsetDateTime::now_utc().format(&Rfc3339)?,
                work_id: args.work_id,
                phase: args.phase,
                tier: args.tier,
                attempt: args.attempt,
                backend: args.backend,
                diagnosis: args.diagnosis,
                intervention: args.intervention,
                tokens: args.tokens,
                elapsed_seconds: args.elapsed_seconds,
                manager_rounds: args.manager_rounds,
                outcome: args.outcome,
                note: args.note,
                owner: args.owner,
            };
            event.validate()?;
            let cfg = config::load(args.config_path.as_deref())?;
            let stored = manager_log::append(&manager_log::path(&cfg.defaults), &event)?;
            println!("{}", serde_json::to_string(&stored)?);
        }
        ManagerLogCommands::Show {
            work_id,
            summary,
            json,
            config_path,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            let log = manager_log::load(&manager_log::path(&cfg.defaults), work_id.as_deref())?;
            if !json && log.skipped_lines > 0 {
                eprintln!(
                    "warning: skipped {} invalid manager log lines",
                    log.skipped_lines
                );
            }
            if summary {
                let result = manager_log::summarize(&log)?;
                if json {
                    println!("{}", serde_json::to_string(&result)?);
                } else {
                    for i in result.items {
                        println!("{} events={} attempts={} manager_rounds={} tokens={} elapsed_seconds={} first_ts={} last_ts={} last_outcome={} last_phase={}", i.work_id, i.events, i.attempts, i.manager_rounds, i.tokens, i.elapsed_seconds, i.first_ts, i.last_ts, i.last_outcome.as_deref().unwrap_or("-"), serde_json::to_value(i.last_phase)?.as_str().unwrap_or("-"));
                    }
                }
            } else if json {
                println!("{}", serde_json::to_string(&log)?);
            } else {
                for e in log.events {
                    let mut line = format!(
                        "{} {} {} attempt={} outcome={}",
                        e.ts,
                        e.work_id,
                        serde_json::to_value(e.phase)?.as_str().unwrap_or("-"),
                        e.attempt
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| "-".into()),
                        e.outcome.as_deref().unwrap_or("-")
                    );
                    if let Some(n) = e.tokens {
                        line.push_str(&format!(" tokens={n}"));
                    }
                    if let Some(n) = e.elapsed_seconds {
                        line.push_str(&format!(" elapsed_seconds={n}"));
                    }
                    println!("{line}");
                }
            }
        }
    }
    Ok(())
}
