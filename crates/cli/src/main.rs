//! The `dasher` command-line tool: loads a page and prints its final HTML,
//! or the value of a JavaScript expression; or runs dasher as an HTTP
//! service or an AWS Lambda runtime.

mod args;
mod run;
mod service;

use std::path::Path;
use std::process::ExitCode;

use args::{Command, USAGE};

fn main() -> ExitCode {
    // A Lambda custom runtime is an executable called `bootstrap`, run
    // without arguments.
    let program = std::env::args_os().next().unwrap_or_default();
    if Path::new(&program).file_name() == Some("bootstrap".as_ref()) {
        return service::lambda();
    }

    let env = |name: &str| std::env::var(name).ok();
    let command = match args::parse(std::env::args().skip(1), &env) {
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
        Command::ServiceHelp => {
            println!("{}", service::USAGE);
            ExitCode::SUCCESS
        }
        Command::Serve(options) => service::serve(&options),
        Command::Lambda => service::lambda(),
    }
}
