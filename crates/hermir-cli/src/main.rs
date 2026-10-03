//! `hermir` on the command line. Nouns first, verbs second, `--json` everywhere: one JSON
//! document on stdout per run, errors included. Human text goes to stdout, progress and what
//! went wrong to stderr; the exit code says which kind of failure it was (`hermir --help`).
use std::io::Write;

use clap::Parser;

mod cli;
mod cmd;
mod parse;
mod render;

fn main() {
    let cli = match cli::Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // A usage error is a run like any other under --json: one document, exit 2.
            if e.use_stderr() && std::env::args().any(|a| a == "--json") {
                let doc = serde_json::json!({ "error": e.to_string().trim(), "exit": 2 });
                println!("{doc}");
                std::process::exit(2);
            }
            e.exit();
        }
    };
    let json = cli.json;
    let outcome = cmd::run(cli).unwrap_or_else(|e| cmd::Outcome::error(&e));
    let mut stdout = std::io::stdout();
    if json {
        let _ = writeln!(
            stdout,
            "{}",
            serde_json::to_string_pretty(&outcome.json).unwrap_or_default()
        );
    } else if !outcome.human.is_empty() {
        let _ = writeln!(stdout, "{}", outcome.human);
    }
    if let Some(note) = &outcome.note {
        eprintln!("hermir: {note}");
    }
    std::process::exit(outcome.exit);
}
