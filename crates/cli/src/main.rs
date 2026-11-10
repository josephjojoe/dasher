//! The `dasher` command-line tool.

use std::process::ExitCode;

const USAGE: &str = "\
usage: dasher [--help | --version]

dasher is a minimal headless browser for scraping. Page loading is not
implemented yet.";

fn main() -> ExitCode {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("-V" | "--version") => {
            println!("dasher {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("-h" | "--help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
