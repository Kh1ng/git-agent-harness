// Command execution for `gah server` (ticket #410).

use anyhow::Result;

pub struct Args {
    pub port: u16,
    pub host: String,
}

pub fn run(args: Args) -> Result<()> {
    println!("Starting WebSocket server on {}:{}", args.host, args.port);
    crate::server::run_blocking(&args.host, args.port)?;
    // `run_blocking` starts a thread and returns immediately.
    std::thread::park();
    Ok(())
}
