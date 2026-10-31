//! `dentalcraft-cli`: drives DentalCraft from the command line and serves
//! MCP for AI agents (port plan B2.1).
//!
//! Planned subcommands: `info`, `run`, `commands`, `describe`, `render`,
//! `app` and `mcp [--connect PORT]`, over a headless engine session
//! (`dc-session`) or a running app started with `--control <port>`.
//! Skeleton only for now: it prints the help.

use clap::{CommandFactory, Parser};

/// Command line of `dentalcraft-cli`.
#[derive(Debug, Parser)]
#[command(
    name = "dentalcraft-cli",
    version,
    about = "Drive DentalCraft from the command line and over MCP"
)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
    // no subcommand exists yet: show what the tool is
    Cli::command().print_help().expect("write help to stdout");
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
        let help = Cli::command().render_help().to_string();
        assert!(help.contains("dentalcraft-cli"), "{help}");
    }
}
