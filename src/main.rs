mod agent;
mod audio;
mod binary;
mod config;
mod frames;
mod resource;
mod transcribe;
mod unique_id;
mod web;

use std::process::ExitCode;

fn main() -> ExitCode {
    match agent::run(std::env::args_os().collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}
