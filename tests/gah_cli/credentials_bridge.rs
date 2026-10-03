use clap::Parser;
use git_agent_harness::cli::args::Cli;

#[path = "../../apps/desktop/credentials/cli_args.rs"]
mod native_args;

#[test]
fn native_provider_connection_commands_match_installed_cli() {
    for args in [
        native_args::list(),
        native_args::save(
            "nous-personal",
            "nous",
            "api_key",
            "Personal",
            Some("NOUS_API_KEY"),
        ),
        native_args::save(
            "mistral-second",
            "mistral",
            "mistral_dashboard",
            "Second account",
            None,
        ),
        native_args::remove("nous-personal"),
        native_args::refresh("mistral-second"),
        native_args::instances(),
        native_args::bind("gah", "vibe-personal", "mistral-personal"),
        native_args::add_instance(
            "gah",
            "vibe-personal",
            "vibe",
            "mistral-personal",
            "Personal",
        ),
    ] {
        let parsed = Cli::try_parse_from(std::iter::once("gah".to_owned()).chain(args));
        assert!(
            parsed.is_ok(),
            "native command must parse: {}",
            parsed.err().map(|e| e.to_string()).unwrap_or_default()
        );
    }
}
