//! The `dasher` command-line tool: loads a page and prints its final HTML,
//! or the value of a JavaScript expression.

mod args;
mod run;

use std::process::ExitCode;

use args::{Command, USAGE};

fn main() -> ExitCode {
    let command = match args::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("dasher: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match command {
        Command::Help => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("dasher {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Run(options) => run::main(&options),
    }
}
