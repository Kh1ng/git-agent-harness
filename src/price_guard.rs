use crate::models::Watchlist;
use anyhow::Result;
use std::fs;

pub fn run(watchlist: &str, model: &str) -> Result<()> {
    let watchlist: Watchlist = serde_json::from_str(&fs::read_to_string(watchlist)?)?;
    let Some(m) = watchlist.models.into_iter().find(|m| m.id == model) else {
        println!("blocked");
        std::process::exit(1);
    };
    if m.status.contains("unavailable")
        || m.input_per_1m > m.max_input_per_1m
        || m.output_per_1m > m.max_output_per_1m
    {
        println!("blocked");
        std::process::exit(1);
    }
    println!("allowed");
    Ok(())
}
