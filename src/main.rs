mod agent;
mod audio;
mod binary;
mod config;
mod frames;
mod local_derive;
mod resource;
mod transcribe;
mod unique_id;
mod web;

use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().collect();
    let result = if arguments
        .get(1)
        .is_some_and(|argument| argument == "_local-derive")
    {
        local_derive::run(arguments.into_iter().skip(1).collect())
    } else {
        agent::run(arguments)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}
