// Binary crate root.
//
// The production module graph lives in `src/lib.rs`; this binary only
// initializes the library-owned CLI entry point. See ticket #406.

fn main() -> anyhow::Result<()> {
    git_agent_harness::cli::run()
}
